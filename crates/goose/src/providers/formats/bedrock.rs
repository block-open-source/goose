use std::borrow::Cow;
use std::collections::HashMap;
use std::path::Path;

use crate::mcp_utils::ToolResult;
use anyhow::{anyhow, bail, Result};
use aws_sdk_bedrockruntime::types as bedrock;
use aws_smithy_types::{Document, Number};
use base64::Engine;
use chrono::Utc;
use rmcp::model::{
    object, CallToolRequestParams, ContentBlock, ErrorCode, ErrorData, ResourceContents, Role, Tool,
};
use serde_json::Value;

use crate::conversation::message::{Message, MessageContent};
use crate::providers::bedrock::BEDROCK_PROVIDER_NAME;
use crate::providers::canonical::maybe_get_canonical_model;
use crate::providers::formats::anthropic::{
    adaptive_output_effort, model_supports_temperature, requires_explicit_thinking_disable,
    thinking_block_is_stale, thinking_budget_tokens, thinking_type_for_provider, ThinkingType,
    ANTHROPIC_PROVIDER_NAME, MIN_ANSWER_TOKENS,
};
use crate::utils::{sanitize_unicode_tags, strip_unicode_tags};
use goose_providers::conversation::token_usage::Usage;
use goose_providers::documents::{unsupported_document_text, UNSUPPORTED_PROVIDER_REASON};
use goose_providers::model::ModelConfig;
use once_cell::sync::Lazy;
use regex::Regex;

static BEDROCK_VERSION_SUFFIX_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"-v\d+(:\d+)?$").unwrap());

pub fn bedrock_anthropic_thinking_fields(model_config: &ModelConfig) -> Option<Document> {
    let anthropic_config = bedrock_anthropic_model_config(model_config)?;
    let thinking_type = thinking_type_for_provider(ANTHROPIC_PROVIDER_NAME, &anthropic_config);
    let thinking = match thinking_type {
        ThinkingType::Adaptive => Document::Object(HashMap::from([(
            "type".to_string(),
            Document::String("adaptive".to_string()),
        )])),
        ThinkingType::Enabled => {
            // Thinking tokens count against `maxTokens`, which `bedrock_inference_config`
            // now sends when explicitly configured. Mirror the Anthropic formatter: clamp
            // the budget to leave room for an answer, and drop thinking entirely when even
            // a minimal budget wouldn't fit under the cap. When max_tokens is unset, Bedrock
            // applies its per-model default so there is nothing to clamp against.
            let mut budget_tokens = thinking_budget_tokens(model_config);
            if let Some(max_tokens) = model_config.max_tokens {
                budget_tokens = budget_tokens.min(max_tokens.saturating_sub(MIN_ANSWER_TOKENS));
                if budget_tokens < MIN_ANSWER_TOKENS {
                    return None;
                }
            }
            Document::Object(HashMap::from([
                ("type".to_string(), Document::String("enabled".to_string())),
                (
                    "budget_tokens".to_string(),
                    Document::Number(Number::PosInt(budget_tokens as u64)),
                ),
            ]))
        }
        ThinkingType::Disabled => {
            if !requires_explicit_thinking_disable(
                ANTHROPIC_PROVIDER_NAME,
                &anthropic_config.model_name,
            ) {
                return None;
            }
            Document::Object(HashMap::from([(
                "type".to_string(),
                Document::String("disabled".to_string()),
            )]))
        }
    };

    let mut fields = HashMap::from([("thinking".to_string(), thinking)]);

    if thinking_type == ThinkingType::Adaptive {
        fields.insert(
            "output_config".to_string(),
            Document::Object(HashMap::from([(
                "effort".to_string(),
                Document::String(adaptive_output_effort(model_config).to_string()),
            )])),
        );
    }

    Some(Document::Object(fields))
}

fn bedrock_anthropic_model_config(model_config: &ModelConfig) -> Option<ModelConfig> {
    let (_, anthropic_model) = model_config.model_name.rsplit_once("anthropic.")?;

    Some(ModelConfig {
        model_name: strip_bedrock_version_suffix(anthropic_model),
        ..model_config.clone()
    })
}

/// Bedrock model ids carry a `-v1:0` style suffix (e.g.
/// `claude-opus-4-1-20250805-v1:0`) that the canonical Anthropic registry does
/// not recognise. Dropping it lets the date stamp become the terminal segment
/// the registry already knows how to normalise.
fn strip_bedrock_version_suffix(model_name: &str) -> String {
    BEDROCK_VERSION_SUFFIX_RE
        .replace(model_name, "")
        .into_owned()
}

/// Build the Bedrock `InferenceConfiguration` (`maxTokens`, `temperature`) for
/// a request from the active [`ModelConfig`].
///
/// Without this the `Converse`/`ConverseStream` APIs fall back to per-model
/// server defaults, so a configured `max_tokens`/`temperature` is silently
/// dropped. Each field is sent only when the user has configured it, so that
/// unset values continue to use Bedrock's per-model server defaults rather than
/// being pinned to a generic fallback:
/// - `max_tokens` is sent only when explicitly set (`model_config.max_tokens`).
///   Using [`ModelConfig::max_output_tokens`] here would forward its `4096`
///   fallback for every model whose id is not in the canonical catalog (e.g.
///   cross-region ids like `us.anthropic.claude-...`), capping models whose
///   real output limit is far higher.
/// - `temperature` is sent only when set and the model supports it. Support is
///   resolved against the Anthropic canonical registry for `anthropic.*` model
///   ids (the same mapping used for thinking) and the Bedrock canonical registry
///   for other known Bedrock ids, so models that reject a custom temperature keep
///   the server default.
pub fn bedrock_inference_config(model_config: &ModelConfig) -> bedrock::InferenceConfiguration {
    let mut builder = bedrock::InferenceConfiguration::builder();

    if let Some(max_tokens) = model_config.max_tokens {
        builder = builder.max_tokens(max_tokens);
    }

    if let Some(temperature) = model_config.temperature {
        if bedrock_model_supports_temperature(model_config) {
            builder = builder.temperature(temperature);
        }
    }

    builder.build()
}

/// Whether `temperature` may be sent for this Bedrock model. For `anthropic.*`
/// ids we resolve against the Anthropic canonical registry; for other known
/// Bedrock ids we consult the Bedrock canonical registry and otherwise keep the
/// permissive fallback used by [`model_supports_temperature`].
fn bedrock_model_supports_temperature(model_config: &ModelConfig) -> bool {
    if let Some(anthropic_config) = bedrock_anthropic_model_config(model_config) {
        model_supports_temperature(ANTHROPIC_PROVIDER_NAME, &anthropic_config)
    } else {
        maybe_get_canonical_model(BEDROCK_PROVIDER_NAME, &model_config.model_name)
            .and_then(|model| model.temperature)
            .unwrap_or(true)
    }
}

pub fn to_bedrock_message_with_caching(
    message: &Message,
    enable_caching: bool,
    current_model: Option<&str>,
) -> Result<bedrock::Message> {
    let thinking_is_stale = thinking_block_is_stale(message, current_model);
    let mut content_blocks: Vec<bedrock::ContentBlock> = message
        .content
        .iter()
        .filter(|content| {
            if !thinking_is_stale {
                return true;
            }
            match content {
                MessageContent::Thinking(thinking) => thinking.signature.is_empty(),
                MessageContent::RedactedThinking(_) => false,
                _ => true,
            }
        })
        .map(to_bedrock_message_content)
        .collect::<Result<_>>()?;

    if enable_caching && !content_blocks.is_empty() {
        content_blocks.push(bedrock::ContentBlock::CachePoint(
            bedrock::CachePointBlock::builder()
                .r#type(bedrock::CachePointType::Default)
                .build()
                .map_err(|e| anyhow!("Failed to build cache point for message: {}", e))?,
        ));
    }

    bedrock::Message::builder()
        .role(to_bedrock_role(&message.role))
        .set_content(Some(content_blocks))
        .build()
        .map_err(|err| anyhow!("Failed to construct Bedrock message: {}", err))
}

pub fn to_bedrock_message_content(content: &MessageContent) -> Result<bedrock::ContentBlock> {
    Ok(match content {
        MessageContent::Text(text) => bedrock::ContentBlock::Text(text.text.to_string()),
        MessageContent::ToolConfirmationRequest(_tool_confirmation_request) => {
            bedrock::ContentBlock::Text("".to_string())
        }
        MessageContent::ActionRequired(_action_required) => {
            bedrock::ContentBlock::Text("".to_string())
        }
        MessageContent::Image(image) => {
            bedrock::ContentBlock::Image(to_bedrock_image(&image.data, &image.mime_type)?)
        }
        MessageContent::Document(document) => bedrock::ContentBlock::Text(
            unsupported_document_text(document, UNSUPPORTED_PROVIDER_REASON),
        ),
        MessageContent::Thinking(thinking) => {
            let mut builder = bedrock::ReasoningTextBlock::builder().text(&thinking.thinking);
            if !thinking.signature.is_empty() {
                builder = builder.signature(&thinking.signature);
            }
            bedrock::ContentBlock::ReasoningContent(bedrock::ReasoningContentBlock::ReasoningText(
                builder.build()?,
            ))
        }
        MessageContent::RedactedThinking(redacted) => {
            match base64::prelude::BASE64_STANDARD.decode(&redacted.data) {
                Ok(bytes) => bedrock::ContentBlock::ReasoningContent(
                    bedrock::ReasoningContentBlock::RedactedContent(aws_smithy_types::Blob::new(
                        bytes,
                    )),
                ),
                Err(_) => bedrock::ContentBlock::Text("".to_string()),
            }
        }
        MessageContent::SystemNotification(_) => {
            bail!("SystemNotification should not get passed to the provider")
        }
        MessageContent::Error(_) => {
            bail!("Error content should not get passed to the provider")
        }
        MessageContent::ToolRequest(tool_req) => {
            let tool_use_id = tool_req.id.to_string();
            let tool_use = if let Ok(call) = tool_req.tool_call.as_ref() {
                bedrock::ToolUseBlock::builder()
                    .tool_use_id(tool_use_id)
                    .name(call.name.to_string())
                    .input(to_bedrock_json(&args_to_value(call.arguments.clone())))
                    .build()
            } else {
                // Unparseable tool call: emit a placeholder tool_use so the paired
                // tool_result isn't orphaned — Bedrock rejects a tool_use with no name
                // and a tool_result with no matching tool_use. Mirrors the
                // OpenAI/Databricks/Anthropic formatters.
                bedrock::ToolUseBlock::builder()
                    .tool_use_id(tool_use_id)
                    .name("unparseable_tool_call")
                    .input(to_bedrock_json(&args_to_value(None)))
                    .build()
            }?;
            bedrock::ContentBlock::ToolUse(tool_use)
        }
        MessageContent::ToolResponse(tool_res) => {
            let content = match &tool_res.tool_result {
                Ok(result) => Some(
                    result
                        .content
                        .iter()
                        .map(|c| to_bedrock_tool_result_content_block(&tool_res.id, c.clone()))
                        .collect::<Result<_>>()?,
                ),
                Err(error) => {
                    let message = format!("The tool call returned the following error:\n{}", error);
                    Some(vec![bedrock::ToolResultContentBlock::Text(
                        crate::utils::sanitize_unicode_tags(&message),
                    )])
                }
            };
            bedrock::ContentBlock::ToolResult(
                bedrock::ToolResultBlock::builder()
                    .tool_use_id(tool_res.id.to_string())
                    .status(if tool_res.tool_result.is_ok() {
                        bedrock::ToolResultStatus::Success
                    } else {
                        bedrock::ToolResultStatus::Error
                    })
                    .set_content(content)
                    .build()?,
            )
        }
    })
}

/// Convert MCP Content to Bedrock ToolResultContentBlock
///
/// Supports text, images, and document resources. Images are supported
/// by Bedrock for Anthropic Claude 3 models.
pub fn to_bedrock_tool_result_content_block(
    tool_use_id: &str,
    content: ContentBlock,
) -> Result<bedrock::ToolResultContentBlock> {
    Ok(match content {
        ContentBlock::Text(text) => bedrock::ToolResultContentBlock::Text(text.text),
        ContentBlock::Image(image) => {
            bedrock::ToolResultContentBlock::Image(to_bedrock_image(&image.data, &image.mime_type)?)
        }
        ContentBlock::ResourceLink(_link) => {
            bedrock::ToolResultContentBlock::Text("[Resource link]".to_string())
        }
        ContentBlock::Resource(resource) => match &resource.resource {
            ResourceContents::TextResourceContents { text, .. } => {
                match to_bedrock_document(tool_use_id, &resource.resource)? {
                    Some(doc) => bedrock::ToolResultContentBlock::Document(doc),
                    None => {
                        bedrock::ToolResultContentBlock::Text(sanitize_unicode_tags(text.as_str()))
                    }
                }
            }
            ResourceContents::BlobResourceContents { .. } => {
                bail!("Blob resource content is not supported by Bedrock provider yet")
            }
            _ => bail!("Unsupported resource content"),
        },
        ContentBlock::Audio(..) => bail!("Audio is not supported by Bedrock provider"),
        _ => bail!("Unsupported content"),
    })
}

pub fn to_bedrock_role(role: &Role) -> bedrock::ConversationRole {
    match role {
        Role::User => bedrock::ConversationRole::User,
        Role::Assistant => bedrock::ConversationRole::Assistant,
    }
}

pub fn to_bedrock_image(data: &str, mime_type: &str) -> Result<bedrock::ImageBlock> {
    // Extract format from MIME type
    let format = match mime_type {
        "image/png" => bedrock::ImageFormat::Png,
        "image/jpeg" | "image/jpg" => bedrock::ImageFormat::Jpeg,
        "image/gif" => bedrock::ImageFormat::Gif,
        "image/webp" => bedrock::ImageFormat::Webp,
        _ => bail!(
            "Unsupported image format: {}. Bedrock supports png, jpeg, gif, webp",
            mime_type
        ),
    };

    // Create image source with base64 data
    let source = bedrock::ImageSource::Bytes(aws_smithy_types::Blob::new(
        base64::prelude::BASE64_STANDARD
            .decode(data)
            .map_err(|e| anyhow!("Failed to decode base64 image data: {}", e))?,
    ));

    // Build the image block
    Ok(bedrock::ImageBlock::builder()
        .format(format)
        .source(source)
        .build()?)
}

pub fn to_bedrock_tool_config(tools: &[Tool]) -> Result<bedrock::ToolConfiguration> {
    Ok(bedrock::ToolConfiguration::builder()
        .set_tools(Some(
            tools.iter().map(to_bedrock_tool).collect::<Result<_>>()?,
        ))
        .build()?)
}

pub fn to_bedrock_tool(tool: &Tool) -> Result<bedrock::Tool> {
    let mut input_schema = tool.input_schema.as_ref().clone();

    // If the schema doesn't have a "type" field, add it
    // This is required by Bedrock
    if !input_schema.contains_key("type") {
        input_schema.insert("type".to_string(), Value::String("object".to_string()));
    }
    let input_schema = sanitize_json_unicode_tags(Value::Object(input_schema))?;

    Ok(bedrock::Tool::ToolSpec(
        bedrock::ToolSpecification::builder()
            .name(tool.name.to_string())
            .description(
                tool.description
                    .as_ref()
                    .map(|d| strip_unicode_tags(d))
                    .unwrap_or_default(),
            )
            .input_schema(bedrock::ToolInputSchema::Json(to_bedrock_json(
                &input_schema,
            )))
            .build()?,
    ))
}

pub(crate) fn sanitize_json_unicode_tags(value: Value) -> Result<Value> {
    Ok(match value {
        Value::String(text) => Value::String(strip_unicode_tags(&text)),
        Value::Array(values) => Value::Array(
            values
                .into_iter()
                .map(sanitize_json_unicode_tags)
                .collect::<Result<_>>()?,
        ),
        Value::Object(values) => {
            let mut sanitized = serde_json::Map::new();
            for (key, value) in values {
                let key = strip_unicode_tags(&key);
                if sanitized.contains_key(&key) {
                    bail!("JSON contains a duplicate key after Unicode tag sanitization");
                }
                sanitized.insert(key, sanitize_json_unicode_tags(value)?);
            }
            Value::Object(sanitized)
        }
        value => value,
    })
}

fn args_to_value(args: Option<serde_json::Map<String, Value>>) -> Value {
    match args {
        Some(map) => Value::Object(map),
        None => Value::Object(serde_json::Map::new()),
    }
}

pub fn to_bedrock_json(value: &Value) -> Document {
    match value {
        Value::Null => Document::Null,
        Value::Bool(bool) => Document::Bool(*bool),
        Value::Number(num) => {
            if let Some(n) = num.as_u64() {
                Document::Number(Number::PosInt(n))
            } else if let Some(n) = num.as_i64() {
                Document::Number(Number::NegInt(n))
            } else if let Some(n) = num.as_f64() {
                Document::Number(Number::Float(n))
            } else {
                unreachable!()
            }
        }
        Value::String(str) => Document::String(str.to_string()),
        Value::Array(arr) => Document::Array(arr.iter().map(to_bedrock_json).collect()),
        Value::Object(obj) => Document::Object(HashMap::from_iter(
            obj.into_iter()
                .map(|(key, val)| (key.to_string(), to_bedrock_json(val))),
        )),
    }
}

fn to_bedrock_document(
    tool_use_id: &str,
    content: &ResourceContents,
) -> Result<Option<bedrock::DocumentBlock>> {
    let (uri, text) = match content {
        ResourceContents::TextResourceContents { uri, text, .. } => {
            (uri, sanitize_unicode_tags(text))
        }
        ResourceContents::BlobResourceContents { .. } => {
            bail!("Blob resource content is not supported by Bedrock provider yet")
        }
        _ => bail!("Unsupported resource content"),
    };

    let filename = Path::new(uri)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(uri);

    // Return None if the file type is not supported
    let (name, format) = match filename.split_once('.') {
        Some((name, "txt")) => (name, bedrock::DocumentFormat::Txt),
        Some((name, "csv")) => (name, bedrock::DocumentFormat::Csv),
        Some((name, "md")) => (name, bedrock::DocumentFormat::Md),
        _ => return Ok(None), // Not a supported document type
    };

    // Since we can't use the full path (due to character limit and also Bedrock does not accept `/` etc.),
    // and Bedrock wants document names to be unique, we're adding `tool_use_id` as a prefix to make
    // document names unique
    let name = format!("{tool_use_id}-{name}");

    Ok(Some(
        bedrock::DocumentBlock::builder()
            .format(format)
            .name(name)
            .source(bedrock::DocumentSource::Bytes(text.as_bytes().into()))
            .build()
            .map_err(|err| anyhow!("Failed to construct Bedrock document: {}", err))?,
    ))
}

pub fn from_bedrock_message(message: &bedrock::Message) -> Result<Message> {
    let role = from_bedrock_role(message.role())?;
    let content = message
        .content()
        .iter()
        .filter(|block| !matches!(block, bedrock::ContentBlock::CachePoint(_)))
        .map(from_bedrock_content_block)
        .collect::<Result<Vec<_>>>()?;
    let created = Utc::now().timestamp();

    Ok(Message::new(role, created, content))
}

pub fn from_bedrock_content_block(block: &bedrock::ContentBlock) -> Result<MessageContent> {
    Ok(match block {
        bedrock::ContentBlock::Text(text) => MessageContent::text(text),
        bedrock::ContentBlock::ToolUse(tool_use) => {
            let arguments = from_bedrock_json(&tool_use.input.clone())
                .and_then(sanitize_json_unicode_tags)
                .map(|arguments| {
                    CallToolRequestParams::new(tool_use.name.clone())
                        .with_arguments(object(arguments))
                })
                .map_err(|error| {
                    ErrorData::new(ErrorCode::INVALID_PARAMS, error.to_string(), None)
                });
            MessageContent::tool_request(tool_use.tool_use_id.to_string(), arguments)
        }
        bedrock::ContentBlock::ToolResult(tool_res) => MessageContent::tool_response(
            tool_res.tool_use_id.to_string(),
            if tool_res.content.is_empty() {
                Err(ErrorData {
                    code: ErrorCode::INTERNAL_ERROR,
                    message: Cow::from("Empty content for tool use from Bedrock".to_string()),
                    data: None,
                })
            } else {
                tool_res
                    .content
                    .iter()
                    .map(from_bedrock_tool_result_content_block)
                    .collect::<ToolResult<Vec<_>>>()
                    .map(rmcp::model::CallToolResult::success)
            },
        ),
        bedrock::ContentBlock::ReasoningContent(reasoning) => {
            from_bedrock_reasoning_content_block(reasoning)?
        }
        bedrock::ContentBlock::CachePoint(_) => {
            bail!("CachePoint blocks should have been filtered out during message processing")
        }
        _ => bail!(
            "Unsupported Bedrock content block type: {}",
            bedrock_content_block_kind(block)
        ),
    })
}

fn from_bedrock_reasoning_content_block(
    reasoning: &bedrock::ReasoningContentBlock,
) -> Result<MessageContent> {
    Ok(match reasoning {
        bedrock::ReasoningContentBlock::ReasoningText(text_block) => {
            let signature = text_block.signature.clone().unwrap_or_default();
            MessageContent::thinking(text_block.text.clone(), signature)
        }
        bedrock::ReasoningContentBlock::RedactedContent(blob) => {
            let encoded = base64::prelude::BASE64_STANDARD.encode(blob.as_ref());
            MessageContent::redacted_thinking(encoded)
        }
        _ => bail!(
            "Unsupported Bedrock reasoning content variant: {}",
            bedrock_reasoning_content_block_kind(reasoning)
        ),
    })
}

fn bedrock_reasoning_content_block_kind(block: &bedrock::ReasoningContentBlock) -> &'static str {
    match block {
        bedrock::ReasoningContentBlock::ReasoningText(_) => "ReasoningText",
        bedrock::ReasoningContentBlock::RedactedContent(_) => "RedactedContent",
        _ => "Unknown",
    }
}

fn bedrock_content_block_kind(block: &bedrock::ContentBlock) -> &'static str {
    match block {
        bedrock::ContentBlock::Audio(_) => "Audio",
        bedrock::ContentBlock::CachePoint(_) => "CachePoint",
        bedrock::ContentBlock::CitationsContent(_) => "CitationsContent",
        bedrock::ContentBlock::Document(_) => "Document",
        bedrock::ContentBlock::GuardContent(_) => "GuardContent",
        bedrock::ContentBlock::Image(_) => "Image",
        bedrock::ContentBlock::ReasoningContent(_) => "ReasoningContent",
        bedrock::ContentBlock::SearchResult(_) => "SearchResult",
        bedrock::ContentBlock::Text(_) => "Text",
        bedrock::ContentBlock::ToolResult(_) => "ToolResult",
        bedrock::ContentBlock::ToolUse(_) => "ToolUse",
        bedrock::ContentBlock::Video(_) => "Video",
        _ => "Unknown",
    }
}

pub fn from_bedrock_tool_result_content_block(
    content: &bedrock::ToolResultContentBlock,
) -> ToolResult<ContentBlock> {
    Ok(match content {
        bedrock::ToolResultContentBlock::Text(text) => ContentBlock::text(text.to_string()),
        _ => {
            return Err(ErrorData {
                code: ErrorCode::INTERNAL_ERROR,
                message: Cow::from("Unsupported tool result from Bedrock".to_string()),
                data: None,
            });
        }
    })
}

pub fn from_bedrock_role(role: &bedrock::ConversationRole) -> Result<Role> {
    Ok(match role {
        bedrock::ConversationRole::User => Role::User,
        bedrock::ConversationRole::Assistant => Role::Assistant,
        _ => bail!("Unknown role from Bedrock"),
    })
}

pub fn from_bedrock_usage(usage: &bedrock::TokenUsage) -> Usage {
    Usage::from_cache_exclusive_input(
        Some(usage.input_tokens),
        Some(usage.output_tokens),
        Some(usage.total_tokens),
        usage.cache_read_input_tokens,
        usage.cache_write_input_tokens,
    )
}

pub fn from_bedrock_json(document: &Document) -> Result<Value> {
    Ok(match document {
        Document::Null => Value::Null,
        Document::Bool(bool) => Value::Bool(*bool),
        Document::Number(num) => match num {
            Number::PosInt(i) => Value::Number((*i).into()),
            Number::NegInt(i) => Value::Number((*i).into()),
            Number::Float(f) => Value::Number(
                serde_json::Number::from_f64(*f).ok_or(anyhow!("Expected a valid float"))?,
            ),
        },
        Document::String(str) => Value::String(str.clone()),
        Document::Array(arr) => {
            Value::Array(arr.iter().map(from_bedrock_json).collect::<Result<_>>()?)
        }
        Document::Object(obj) => Value::Object(
            obj.iter()
                .map(|(key, val)| Ok((key.clone(), from_bedrock_json(val)?)))
                .collect::<Result<_>>()?,
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use goose_test_support::TEST_IMAGE_B64;
    use rmcp::model::ImageContent;
    use serde_json::json;

    #[test]
    fn test_bedrock_anthropic_thinking_fields_enabled() {
        let mut params = HashMap::new();
        params.insert("thinking_effort".to_string(), json!("low"));
        let mut config = ModelConfig::new("us.anthropic.claude-3-7-sonnet-20250219-v1:0");
        config.request_params = Some(params);
        config.reasoning = Some(true);

        let fields = bedrock_anthropic_thinking_fields(&config).expect("thinking fields");
        assert_eq!(
            from_bedrock_json(&fields).unwrap(),
            json!({
                "thinking": {
                    "type": "enabled",
                    "budget_tokens": 4000
                }
            })
        );
    }

    #[test]
    fn test_bedrock_anthropic_thinking_fields_clamped_to_max_tokens() {
        // budget (4000) exceeds the room left under an explicit max_tokens, so it
        // is clamped to max_tokens - MIN_ANSWER_TOKENS, matching the Anthropic
        // formatter. Without max_tokens set there is nothing to clamp against.
        let mut params = HashMap::new();
        params.insert("thinking_effort".to_string(), json!("low"));
        let mut config = ModelConfig::new("us.anthropic.claude-3-7-sonnet-20250219-v1:0");
        config.request_params = Some(params);
        config.reasoning = Some(true);
        config.max_tokens = Some(3000);

        let fields = bedrock_anthropic_thinking_fields(&config).expect("thinking fields");
        assert_eq!(
            from_bedrock_json(&fields).unwrap(),
            json!({
                "thinking": {
                    "type": "enabled",
                    "budget_tokens": 3000 - 1024
                }
            })
        );
    }

    #[test]
    fn test_bedrock_anthropic_thinking_fields_dropped_when_no_room() {
        // When even a minimal budget wouldn't leave MIN_ANSWER_TOKENS under the
        // cap, thinking is dropped rather than emitting an unsatisfiable request.
        let mut params = HashMap::new();
        params.insert("thinking_effort".to_string(), json!("low"));
        let mut config = ModelConfig::new("us.anthropic.claude-3-7-sonnet-20250219-v1:0");
        config.request_params = Some(params);
        config.reasoning = Some(true);
        config.max_tokens = Some(1500);

        assert!(bedrock_anthropic_thinking_fields(&config).is_none());
    }

    #[test]
    fn test_bedrock_anthropic_thinking_fields_disabled() {
        let mut config = ModelConfig::new("us.anthropic.claude-3-7-sonnet-20250219-v1:0");
        config.reasoning = Some(true);
        config.request_params = Some(HashMap::from([(
            "thinking_effort".to_string(),
            json!("off"),
        )]));

        assert!(bedrock_anthropic_thinking_fields(&config).is_none());

        config.model_name = "us.anthropic.claude-opus-4-7-20251101-v1:0".to_string();
        let fields = bedrock_anthropic_thinking_fields(&config).expect("thinking fields");
        assert_eq!(
            from_bedrock_json(&fields).unwrap(),
            json!({ "thinking": {"type": "disabled"} })
        );
    }

    #[test]
    fn test_bedrock_anthropic_thinking_fields_always_on_adaptive() {
        let mut config = ModelConfig::new("global.anthropic.claude-fable-5");
        config.reasoning = Some(true);
        config.request_params = Some(HashMap::from([(
            "thinking_effort".to_string(),
            json!("off"),
        )]));

        let fields = bedrock_anthropic_thinking_fields(&config).expect("thinking fields");
        assert_eq!(
            from_bedrock_json(&fields).unwrap(),
            json!({
                "thinking": {"type": "adaptive"},
                "output_config": {"effort": "high"}
            })
        );
    }

    #[test]
    fn test_bedrock_anthropic_thinking_fields_adaptive_with_effort() {
        let mut config = ModelConfig::new("us.anthropic.claude-opus-4.7");
        config.reasoning = Some(true);
        config.request_params = Some(HashMap::from([(
            "thinking_effort".to_string(),
            json!("low"),
        )]));

        let fields = bedrock_anthropic_thinking_fields(&config).expect("thinking fields");
        assert_eq!(
            from_bedrock_json(&fields).unwrap(),
            json!({
                "thinking": {"type": "adaptive"},
                "output_config": {"effort": "low"}
            })
        );
    }

    #[test]
    fn test_bedrock_anthropic_thinking_fields_adaptive_with_version_suffix() {
        let mut config = ModelConfig::new("us.anthropic.claude-opus-4-7-20251101-v1:0");
        config.reasoning = Some(true);
        config.request_params = Some(HashMap::from([(
            "thinking_effort".to_string(),
            json!("low"),
        )]));

        let fields = bedrock_anthropic_thinking_fields(&config).expect("thinking fields");
        assert_eq!(
            from_bedrock_json(&fields).unwrap(),
            json!({
                "thinking": {"type": "adaptive"},
                "output_config": {"effort": "low"}
            })
        );
    }

    #[test]
    fn test_bedrock_thinking_fields_skipped_for_non_anthropic() {
        let mut config = ModelConfig::new("us.deepseek.r1-v1:0");
        config.reasoning = Some(true);
        config.request_params = Some(HashMap::from([(
            "thinking_effort".to_string(),
            json!("low"),
        )]));

        assert!(bedrock_anthropic_thinking_fields(&config).is_none());
    }

    #[test]
    fn test_to_bedrock_image_supported_formats() -> Result<()> {
        let supported_formats = [
            "image/png",
            "image/jpeg",
            "image/jpg",
            "image/gif",
            "image/webp",
        ];

        for mime_type in supported_formats {
            let image = ImageContent::new(TEST_IMAGE_B64.to_string(), mime_type.to_string());

            let result = to_bedrock_image(&image.data, &image.mime_type);
            assert!(result.is_ok(), "Failed to convert {} format", mime_type);
        }

        Ok(())
    }

    #[test]
    fn test_to_bedrock_image_unsupported_format() {
        let image = ImageContent::new(TEST_IMAGE_B64.to_string(), "image/bmp".to_string());

        let result = to_bedrock_image(&image.data, &image.mime_type);
        assert!(result.is_err());
        let error_msg = result.unwrap_err().to_string();
        assert!(error_msg.contains("Unsupported image format: image/bmp"));
        assert!(error_msg.contains("Bedrock supports png, jpeg, gif, webp"));
    }

    #[test]
    fn test_to_bedrock_image_invalid_base64() {
        let image = ImageContent::new(
            "invalid_base64_data!!!".to_string(),
            "image/png".to_string(),
        );

        let result = to_bedrock_image(&image.data, &image.mime_type);
        assert!(result.is_err());
        let error_msg = result.unwrap_err().to_string();
        assert!(error_msg.contains("Failed to decode base64 image data"));
    }

    #[test]
    fn test_to_bedrock_tool_sanitizes_description_and_schema_metadata() -> Result<()> {
        let tool = Tool::new(
            "lookup",
            "検索\u{E0041} ツール cafe\u{301}",
            serde_json::Map::from_iter([
                ("type".to_string(), json!("object")),
                (
                    "properties".to_string(),
                    json!({
                        "pro\u{E0042}mpt": {
                            "type": "string",
                            "description": "都市🌍\u{E0043}"
                        }
                    }),
                ),
            ]),
        );

        let bedrock_tool = to_bedrock_tool(&tool)?;
        let spec = bedrock_tool
            .as_tool_spec()
            .expect("expected Bedrock tool specification");
        assert_eq!(spec.description(), Some("検索 ツール cafe\u{301}"));

        let schema = spec
            .input_schema()
            .expect("expected input schema")
            .as_json()
            .expect("expected JSON input schema");
        assert_eq!(
            from_bedrock_json(schema)?,
            json!({
                "type": "object",
                "properties": {
                    "prompt": {
                        "type": "string",
                        "description": "都市🌍"
                    }
                }
            })
        );

        Ok(())
    }

    #[test]
    fn test_to_bedrock_tool_rejects_schema_key_collision_after_sanitization() {
        let mut properties = serde_json::Map::new();
        properties.insert("prompt".to_string(), json!({"type": "string"}));
        properties.insert("pro\u{E0041}mpt".to_string(), json!({"type": "string"}));
        let tool = Tool::new(
            "lookup",
            "Lookup",
            serde_json::Map::from_iter([
                ("type".to_string(), json!("object")),
                ("properties".to_string(), Value::Object(properties)),
            ]),
        );

        let error = to_bedrock_tool(&tool).expect_err("sanitized keys must remain unique");
        assert!(error.to_string().contains("duplicate key"));
    }

    #[test]
    fn test_to_bedrock_message_content_image() -> Result<()> {
        let image = ImageContent::new(TEST_IMAGE_B64.to_string(), "image/png".to_string());

        let message_content = MessageContent::Image(image);
        let result = to_bedrock_message_content(&message_content)?;

        // Verify we get an Image content block
        assert!(matches!(result, bedrock::ContentBlock::Image(_)));

        Ok(())
    }

    #[test]
    fn test_from_bedrock_tool_use_sanitizes_nested_arguments() -> Result<()> {
        let original = bedrock::ContentBlock::ToolUse(
            bedrock::ToolUseBlock::builder()
                .tool_use_id("tool-1")
                .name("lookup")
                .input(to_bedrock_json(&json!({
                    "query": "visible\u{E0041}text",
                    "nested": [{"cit\u{E0042}y": "東京🌍\u{E0043}"}],
                    "path": "cafe\u{301}.txt"
                })))
                .build()?,
        );

        let MessageContent::ToolRequest(request) = from_bedrock_content_block(&original)? else {
            panic!("expected tool request");
        };
        let call = request.tool_call.expect("expected valid tool call");
        assert_eq!(
            call.arguments,
            Some(object(json!({
                "query": "visibletext",
                "nested": [{"city": "東京🌍"}],
                "path": "cafe\u{301}.txt"
            })))
        );

        Ok(())
    }

    #[test]
    fn test_from_bedrock_tool_use_collision_yields_error_request() -> Result<()> {
        let original = bedrock::ContentBlock::ToolUse(
            bedrock::ToolUseBlock::builder()
                .tool_use_id("tool-1")
                .name("lookup")
                .input(to_bedrock_json(&json!({
                    "prompt": "visible",
                    "pro\u{E0041}mpt": "hidden"
                })))
                .build()?,
        );

        let MessageContent::ToolRequest(request) = from_bedrock_content_block(&original)? else {
            panic!("expected tool request");
        };
        let error = request
            .tool_call
            .expect_err("sanitized key collision must produce an invalid tool call");
        assert_eq!(error.code, ErrorCode::INVALID_PARAMS);
        assert!(error.message.contains("duplicate key"));

        Ok(())
    }

    #[test]
    fn test_to_bedrock_tool_result_content_block_image() -> Result<()> {
        let content = ContentBlock::image(TEST_IMAGE_B64.to_string(), "image/png".to_string());
        let result = to_bedrock_tool_result_content_block("test_id", content)?;

        // Verify the wrapper correctly converts ContentBlock::Image to ToolResultContentBlock::Image
        assert!(matches!(result, bedrock::ToolResultContentBlock::Image(_)));

        Ok(())
    }

    #[test]
    fn test_to_bedrock_tool_result_sanitizes_text_resource_fallback() -> Result<()> {
        let content = ContentBlock::embedded_text("file:///result.bin", "visible\u{E0041}text");
        let result = to_bedrock_tool_result_content_block("test_id", content)?;

        let bedrock::ToolResultContentBlock::Text(text) = result else {
            panic!("expected text fallback");
        };
        assert_eq!(text, "visibletext");

        Ok(())
    }

    #[test]
    fn test_to_bedrock_tool_result_sanitizes_document_resource() -> Result<()> {
        let content = ContentBlock::embedded_text("file:///result.txt", "visible\u{E0041}text");
        let result = to_bedrock_tool_result_content_block("test_id", content)?;

        let bedrock::ToolResultContentBlock::Document(document) = result else {
            panic!("expected document");
        };
        let Some(bedrock::DocumentSource::Bytes(bytes)) = document.source() else {
            panic!("expected document bytes");
        };
        assert_eq!(bytes.as_ref(), b"visibletext");

        Ok(())
    }

    #[test]
    fn test_to_bedrock_message_with_caching() -> Result<()> {
        use chrono::Utc;
        use rmcp::model::Role;

        // Multiple content blocks: cache point appended at end, order preserved
        let message = Message::new(
            Role::User,
            Utc::now().timestamp(),
            vec![
                MessageContent::text("First text"),
                MessageContent::text("Second text"),
            ],
        );
        let bedrock_message = to_bedrock_message_with_caching(&message, true, None)?;
        assert_eq!(bedrock_message.content.len(), 3);
        if let bedrock::ContentBlock::Text(text) = &bedrock_message.content[0] {
            assert_eq!(text, "First text");
        } else {
            panic!("Expected text content block");
        }
        if let bedrock::ContentBlock::Text(text) = &bedrock_message.content[1] {
            assert_eq!(text, "Second text");
        } else {
            panic!("Expected text content block");
        }
        assert!(matches!(
            bedrock_message.content[2],
            bedrock::ContentBlock::CachePoint(_)
        ));

        // Caching disabled: no cache point added
        let no_cache = to_bedrock_message_with_caching(&message, false, None)?;
        assert_eq!(no_cache.content.len(), 2);
        for block in &no_cache.content {
            assert!(!matches!(block, bedrock::ContentBlock::CachePoint(_)));
        }

        // Empty content: no cache point added even with caching enabled
        let empty = Message::new(Role::User, Utc::now().timestamp(), vec![]);
        let empty_msg = to_bedrock_message_with_caching(&empty, true, None)?;
        assert_eq!(empty_msg.content.len(), 0);

        Ok(())
    }

    fn signed_thinking_from_model(model: &str) -> Message {
        use crate::conversation::message::InferenceMetadata;

        Message::assistant()
            .with_content(MessageContent::thinking("internal", "sig-abc"))
            .with_text("answer")
            .with_inference(InferenceMetadata {
                provider: "aws_bedrock".to_string(),
                requested_model: model.to_string(),
                resolved_model: None,
                provider_session_id: None,
            })
    }

    #[test]
    fn keeps_signed_thinking_from_the_same_model() -> Result<()> {
        let message = signed_thinking_from_model("anthropic.claude-sonnet-4");
        let formatted =
            to_bedrock_message_with_caching(&message, false, Some("anthropic.claude-sonnet-4"))?;

        assert!(matches!(
            formatted.content[0],
            bedrock::ContentBlock::ReasoningContent(_)
        ));
        assert!(matches!(
            formatted.content[1],
            bedrock::ContentBlock::Text(_)
        ));
        Ok(())
    }

    #[test]
    fn drops_signed_thinking_from_a_different_model() -> Result<()> {
        let message = signed_thinking_from_model("anthropic.claude-opus-4");
        let formatted =
            to_bedrock_message_with_caching(&message, false, Some("anthropic.claude-sonnet-4"))?;

        assert_eq!(formatted.content.len(), 1);
        assert!(matches!(
            formatted.content[0],
            bedrock::ContentBlock::Text(_)
        ));
        Ok(())
    }

    #[test]
    fn test_from_bedrock_usage_folds_cache_tokens_into_input() {
        let usage = bedrock::TokenUsage::builder()
            .input_tokens(7)
            .output_tokens(50)
            .total_tokens(57)
            .cache_read_input_tokens(5000)
            .cache_write_input_tokens(1000)
            .build()
            .unwrap();

        let converted = from_bedrock_usage(&usage);
        assert_eq!(converted.input_tokens, Some(6007));
        assert_eq!(converted.output_tokens, Some(50));
        assert_eq!(converted.total_tokens, Some(6057));
        assert_eq!(converted.cache_read_input_tokens, Some(5000));
        assert_eq!(converted.cache_write_input_tokens, Some(1000));
    }

    #[test]
    fn test_from_bedrock_content_block_cache_point() {
        // Create a cache point block with the required type field
        let cache_point = bedrock::CachePointBlock::builder()
            .r#type(bedrock::CachePointType::Default)
            .build()
            .unwrap();
        let content_block = bedrock::ContentBlock::CachePoint(cache_point);

        // Verify that converting a cache point results in an error
        let result = from_bedrock_content_block(&content_block);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("CachePoint blocks should have been filtered out"));
    }

    #[test]
    fn test_from_bedrock_content_block_reasoning_text() -> Result<()> {
        let reasoning_text = bedrock::ReasoningTextBlock::builder()
            .text("step-by-step reasoning")
            .signature("sig-token")
            .build()?;
        let content_block = bedrock::ContentBlock::ReasoningContent(
            bedrock::ReasoningContentBlock::ReasoningText(reasoning_text),
        );

        match from_bedrock_content_block(&content_block)? {
            MessageContent::Thinking(thinking) => {
                assert_eq!(thinking.thinking, "step-by-step reasoning");
                assert_eq!(thinking.signature, "sig-token");
            }
            other => panic!("Expected Thinking content, got {:?}", other),
        }
        Ok(())
    }

    #[test]
    fn test_from_bedrock_content_block_reasoning_text_without_signature() -> Result<()> {
        let reasoning_text = bedrock::ReasoningTextBlock::builder()
            .text("reasoning without signature")
            .build()?;
        let content_block = bedrock::ContentBlock::ReasoningContent(
            bedrock::ReasoningContentBlock::ReasoningText(reasoning_text),
        );

        match from_bedrock_content_block(&content_block)? {
            MessageContent::Thinking(thinking) => {
                assert_eq!(thinking.thinking, "reasoning without signature");
                assert_eq!(thinking.signature, "");
            }
            other => panic!("Expected Thinking content, got {:?}", other),
        }
        Ok(())
    }

    #[test]
    fn test_from_bedrock_content_block_unsupported_type_errors() {
        let image_block = bedrock::ImageBlock::builder()
            .format(bedrock::ImageFormat::Png)
            .source(bedrock::ImageSource::Bytes(aws_smithy_types::Blob::new(
                Vec::new(),
            )))
            .build()
            .unwrap();
        let content_block = bedrock::ContentBlock::Image(image_block);

        let err = from_bedrock_content_block(&content_block)
            .expect_err("unsupported variant should error");
        let msg = err.to_string();
        assert!(
            msg.contains("Unsupported Bedrock content block type"),
            "got: {msg}"
        );
        assert!(msg.contains("Image"), "got: {msg}");
    }

    #[test]
    fn test_from_bedrock_content_block_reasoning_redacted_content() -> Result<()> {
        let raw = b"encrypted-reasoning-bytes";
        let blob = aws_smithy_types::Blob::new(raw.to_vec());
        let content_block = bedrock::ContentBlock::ReasoningContent(
            bedrock::ReasoningContentBlock::RedactedContent(blob),
        );

        match from_bedrock_content_block(&content_block)? {
            MessageContent::RedactedThinking(redacted) => {
                let expected = base64::prelude::BASE64_STANDARD.encode(raw);
                assert_eq!(redacted.data, expected);
            }
            other => panic!("Expected RedactedThinking content, got {:?}", other),
        }
        Ok(())
    }

    #[test]
    fn test_to_bedrock_message_content_thinking() -> Result<()> {
        let message_content = MessageContent::thinking("because of X", "sig-abc");
        let block = to_bedrock_message_content(&message_content)?;

        match block {
            bedrock::ContentBlock::ReasoningContent(
                bedrock::ReasoningContentBlock::ReasoningText(text_block),
            ) => {
                assert_eq!(text_block.text, "because of X");
                assert_eq!(text_block.signature.as_deref(), Some("sig-abc"));
            }
            other => panic!(
                "Expected ReasoningContentBlock::ReasoningText, got {:?}",
                other
            ),
        }
        Ok(())
    }

    #[test]
    fn test_to_bedrock_message_content_thinking_without_signature() -> Result<()> {
        let message_content = MessageContent::thinking("silent reasoning", "");
        let block = to_bedrock_message_content(&message_content)?;

        match block {
            bedrock::ContentBlock::ReasoningContent(
                bedrock::ReasoningContentBlock::ReasoningText(text_block),
            ) => {
                assert_eq!(text_block.text, "silent reasoning");
                assert!(text_block.signature.is_none());
            }
            other => panic!(
                "Expected ReasoningContentBlock::ReasoningText, got {:?}",
                other
            ),
        }
        Ok(())
    }

    #[test]
    fn test_to_bedrock_message_content_redacted_thinking() -> Result<()> {
        let raw = b"encrypted-reasoning-bytes";
        let encoded = base64::prelude::BASE64_STANDARD.encode(raw);
        let message_content = MessageContent::redacted_thinking(encoded);

        let block = to_bedrock_message_content(&message_content)?;
        match block {
            bedrock::ContentBlock::ReasoningContent(
                bedrock::ReasoningContentBlock::RedactedContent(blob),
            ) => {
                assert_eq!(blob.as_ref(), raw);
            }
            other => panic!(
                "Expected ReasoningContentBlock::RedactedContent, got {:?}",
                other
            ),
        }
        Ok(())
    }

    #[test]
    fn test_to_bedrock_message_content_redacted_thinking_opaque_payload() -> Result<()> {
        let message_content = MessageContent::redacted_thinking("opaque_not_base64!@#".to_string());

        let block = to_bedrock_message_content(&message_content)?;
        match block {
            bedrock::ContentBlock::Text(text) => assert_eq!(text, ""),
            other => panic!(
                "Expected fallback empty Text block for opaque payload, got {:?}",
                other
            ),
        }
        Ok(())
    }

    #[test]
    fn test_bedrock_thinking_round_trip() -> Result<()> {
        let original =
            bedrock::ContentBlock::ReasoningContent(bedrock::ReasoningContentBlock::ReasoningText(
                bedrock::ReasoningTextBlock::builder()
                    .text("chain of thought")
                    .signature("sig-xyz")
                    .build()?,
            ));

        let message_content = from_bedrock_content_block(&original)?;
        let round_tripped = to_bedrock_message_content(&message_content)?;

        match round_tripped {
            bedrock::ContentBlock::ReasoningContent(
                bedrock::ReasoningContentBlock::ReasoningText(text_block),
            ) => {
                assert_eq!(text_block.text, "chain of thought");
                assert_eq!(text_block.signature.as_deref(), Some("sig-xyz"));
            }
            other => panic!(
                "Expected ReasoningContentBlock::ReasoningText, got {:?}",
                other
            ),
        }
        Ok(())
    }

    #[test]
    fn test_bedrock_redacted_thinking_round_trip() -> Result<()> {
        let raw = b"encrypted-reasoning-bytes";
        let original = bedrock::ContentBlock::ReasoningContent(
            bedrock::ReasoningContentBlock::RedactedContent(aws_smithy_types::Blob::new(
                raw.to_vec(),
            )),
        );

        let message_content = from_bedrock_content_block(&original)?;
        let round_tripped = to_bedrock_message_content(&message_content)?;

        match round_tripped {
            bedrock::ContentBlock::ReasoningContent(
                bedrock::ReasoningContentBlock::RedactedContent(blob),
            ) => {
                assert_eq!(blob.as_ref(), raw);
            }
            other => panic!(
                "Expected ReasoningContentBlock::RedactedContent, got {:?}",
                other
            ),
        }
        Ok(())
    }

    #[test]
    fn test_from_bedrock_message_includes_reasoning_content() -> Result<()> {
        use rmcp::model::Role;

        let reasoning_text = bedrock::ReasoningTextBlock::builder()
            .text("thinking out loud")
            .signature("sig")
            .build()?;

        let bedrock_message = bedrock::Message::builder()
            .role(bedrock::ConversationRole::Assistant)
            .content(bedrock::ContentBlock::ReasoningContent(
                bedrock::ReasoningContentBlock::ReasoningText(reasoning_text),
            ))
            .content(bedrock::ContentBlock::Text("final answer".to_string()))
            .build()
            .unwrap();

        let message = from_bedrock_message(&bedrock_message)?;

        assert_eq!(message.role, Role::Assistant);
        assert_eq!(message.content.len(), 2);
        match &message.content[0] {
            MessageContent::Thinking(thinking) => {
                assert_eq!(thinking.thinking, "thinking out loud");
                assert_eq!(thinking.signature, "sig");
            }
            other => panic!("Expected Thinking content, got {:?}", other),
        }
        match &message.content[1] {
            MessageContent::Text(text) => assert_eq!(text.text, "final answer"),
            other => panic!("Expected Text content, got {:?}", other),
        }

        Ok(())
    }

    #[test]
    fn test_from_bedrock_message_filters_cache_points() -> Result<()> {
        use rmcp::model::Role;

        // Create a Bedrock message with mixed content including CachePoint
        let cache_point = bedrock::CachePointBlock::builder()
            .r#type(bedrock::CachePointType::Default)
            .build()
            .unwrap();

        let bedrock_message = bedrock::Message::builder()
            .role(bedrock::ConversationRole::Assistant)
            .content(bedrock::ContentBlock::Text("First text".to_string()))
            .content(bedrock::ContentBlock::CachePoint(cache_point))
            .content(bedrock::ContentBlock::Text("Second text".to_string()))
            .build()
            .unwrap();

        // Convert from Bedrock format
        let message = from_bedrock_message(&bedrock_message)?;

        // Verify that CachePoint was filtered out and only text content remains
        assert_eq!(message.content.len(), 2);
        assert_eq!(message.role, Role::Assistant);

        if let MessageContent::Text(text) = &message.content[0] {
            assert_eq!(text.text, "First text");
        } else {
            panic!("Expected first text content");
        }

        if let MessageContent::Text(text) = &message.content[1] {
            assert_eq!(text.text, "Second text");
        } else {
            panic!("Expected second text content");
        }

        Ok(())
    }

    #[test]
    fn test_cache_points_with_tool_request_messages() -> Result<()> {
        use chrono::Utc;
        use rmcp::model::{CallToolRequestParams, Role};
        use serde_json::json;

        let message = Message::new(
            Role::Assistant,
            Utc::now().timestamp(),
            vec![
                MessageContent::text("I'll use a tool"),
                MessageContent::tool_request(
                    "tool_1".to_string(),
                    Ok(CallToolRequestParams::new("test_tool")
                        .with_arguments(object(json!({"param": "value"})))),
                ),
            ],
        );

        let bedrock_message = to_bedrock_message_with_caching(&message, true, None)?;

        // Verify cache point is added after all content blocks (text + tool request + cache point)
        assert_eq!(bedrock_message.content.len(), 3);
        assert!(matches!(
            bedrock_message.content[0],
            bedrock::ContentBlock::Text(_)
        ));
        assert!(matches!(
            bedrock_message.content[1],
            bedrock::ContentBlock::ToolUse(_)
        ));
        assert!(matches!(
            bedrock_message.content[2],
            bedrock::ContentBlock::CachePoint(_)
        ));

        Ok(())
    }

    #[test]
    fn tool_request_parse_error_gets_placeholder_name() -> Result<()> {
        use rmcp::model::{ErrorCode, ErrorData};
        // An unparseable tool call (ToolRequest(Err)) must still produce a tool_use
        // with a non-empty name; otherwise Bedrock rejects the tool_use / orphans the
        // paired tool_result. Mirrors the OpenAI/Databricks/Anthropic formatters.
        let err = ErrorData::new(
            ErrorCode::INVALID_PARAMS,
            "Tool arguments must be a JSON object".to_string(),
            None,
        );
        let content = MessageContent::tool_request("call_bad".to_string(), Err(err));
        match to_bedrock_message_content(&content)? {
            bedrock::ContentBlock::ToolUse(tu) => {
                assert_eq!(tu.tool_use_id, "call_bad");
                assert_eq!(tu.name, "unparseable_tool_call");
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
        Ok(())
    }

    #[test]
    fn tool_response_error_sanitizes_unicode_tags() -> Result<()> {
        let error = ErrorData::new(
            ErrorCode::INTERNAL_ERROR,
            "visible\u{E0041}\u{E0042} error".to_string(),
            None,
        );
        let content = MessageContent::tool_response("call_hidden".to_string(), Err(error));

        let bedrock::ContentBlock::ToolResult(result) = to_bedrock_message_content(&content)?
        else {
            panic!("expected ToolResult");
        };
        let bedrock::ToolResultContentBlock::Text(text) = &result.content[0] else {
            panic!("expected text error content");
        };

        assert!(!crate::utils::contains_unicode_tags(text.as_str()));
        assert!(text.contains("visible error"));
        Ok(())
    }

    #[test]
    fn tool_response_error_preserves_ordinary_text() -> Result<()> {
        let error = ErrorData::new(
            ErrorCode::INTERNAL_ERROR,
            "ordinary tool failure".to_string(),
            None,
        );
        let content = MessageContent::tool_response("call_error".to_string(), Err(error));

        let bedrock::ContentBlock::ToolResult(result) = to_bedrock_message_content(&content)?
        else {
            panic!("expected ToolResult");
        };
        let bedrock::ToolResultContentBlock::Text(text) = &result.content[0] else {
            panic!("expected text error content");
        };

        assert!(text.contains("ordinary tool failure"));
        Ok(())
    }

    #[test]
    fn test_cache_points_with_tool_response_messages() -> Result<()> {
        use chrono::Utc;
        use rmcp::model::{CallToolResult, Role};

        let message = Message::new(
            Role::User,
            Utc::now().timestamp(),
            vec![MessageContent::tool_response(
                "tool_1".to_string(),
                Ok(CallToolResult::success(vec![ContentBlock::text(
                    "Tool result text".to_string(),
                )])),
            )],
        );

        let bedrock_message = to_bedrock_message_with_caching(&message, true, None)?;

        // Verify cache point is added after tool response content
        assert_eq!(bedrock_message.content.len(), 2);
        assert!(matches!(
            bedrock_message.content[0],
            bedrock::ContentBlock::ToolResult(_)
        ));
        assert!(matches!(
            bedrock_message.content[1],
            bedrock::ContentBlock::CachePoint(_)
        ));

        Ok(())
    }

    #[test]
    fn test_cache_points_with_mixed_tool_content() -> Result<()> {
        use chrono::Utc;
        use rmcp::model::{CallToolRequestParams, Role};
        use serde_json::json;

        let message = Message::new(
            Role::Assistant,
            Utc::now().timestamp(),
            vec![
                MessageContent::text("Using tools"),
                MessageContent::tool_request(
                    "tool_1".to_string(),
                    Ok(CallToolRequestParams::new("tool_a")
                        .with_arguments(object(json!({"key": "val"})))),
                ),
                MessageContent::tool_request(
                    "tool_2".to_string(),
                    Ok(CallToolRequestParams::new("tool_b")
                        .with_arguments(object(json!({"key": "val"})))),
                ),
            ],
        );

        let bedrock_message = to_bedrock_message_with_caching(&message, true, None)?;

        // Verify cache point is added at the end after all tool requests
        assert_eq!(bedrock_message.content.len(), 4);
        assert!(matches!(
            bedrock_message.content[0],
            bedrock::ContentBlock::Text(_)
        ));
        assert!(matches!(
            bedrock_message.content[1],
            bedrock::ContentBlock::ToolUse(_)
        ));
        assert!(matches!(
            bedrock_message.content[2],
            bedrock::ContentBlock::ToolUse(_)
        ));
        assert!(matches!(
            bedrock_message.content[3],
            bedrock::ContentBlock::CachePoint(_)
        ));

        Ok(())
    }

    #[test]
    fn test_bedrock_inference_config_sets_max_tokens_and_temperature() {
        let mut config = ModelConfig::new("us.anthropic.claude-sonnet-4-5-20250929-v1:0");
        config.max_tokens = Some(8192);
        config.temperature = Some(0.5);

        let inference_config = bedrock_inference_config(&config);

        assert_eq!(inference_config.max_tokens(), Some(8192));
        assert_eq!(inference_config.temperature(), Some(0.5));
    }

    #[test]
    fn test_bedrock_inference_config_omits_max_tokens_without_config() {
        let mut config = ModelConfig::new("us.anthropic.claude-sonnet-4-5-20250929-v1:0");
        config.max_tokens = None;
        config.temperature = None;

        let inference_config = bedrock_inference_config(&config);

        // When max_tokens is not explicitly configured we leave it unset so
        // Bedrock applies its per-model server default. Forwarding
        // ModelConfig::max_output_tokens() here would pin every model without a
        // canonical-catalog entry (e.g. cross-region ids) to the generic 4096
        // fallback, capping models whose real output limit is much higher.
        assert_eq!(inference_config.max_tokens(), None);
        assert_eq!(inference_config.temperature(), None);
    }

    #[test]
    fn test_bedrock_inference_config_sends_explicit_max_tokens() {
        let mut config = ModelConfig::new("us.anthropic.claude-sonnet-4-5-20250929-v1:0");
        config.max_tokens = Some(4096);

        let inference_config = bedrock_inference_config(&config);

        // An explicitly configured value is always forwarded.
        assert_eq!(inference_config.max_tokens(), Some(4096));
    }

    #[test]
    fn test_bedrock_inference_config_omits_temperature_for_unsupported_model() {
        let mut config = ModelConfig::new("global.anthropic.claude-sonnet-5");
        config.temperature = Some(0.5);

        let inference_config = bedrock_inference_config(&config);

        assert!(!bedrock_model_supports_temperature(&config));
        assert_eq!(inference_config.temperature(), None);
    }
}
