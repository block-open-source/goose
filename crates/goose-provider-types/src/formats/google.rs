use crate::conversation::token_usage::{ProviderUsage, Usage};
use crate::documents::{
    document_media_type_is_supported, unsupported_document_text, UNSUPPORTED_MEDIA_TYPE_REASON,
};
use crate::errors::ProviderError;
use crate::formats::openai::{is_valid_function_name, sanitize_function_name};
use crate::mcp_utils::extract_text_from_resource;
use crate::model::ModelConfig;
use crate::thinking::ThinkingEffort;
use anyhow::Result;
use rmcp::model::{
    object, CallToolRequestParams, ContentBlock, ErrorCode, ErrorData, ResourceContents, Role, Tool,
};
use serde::Serialize;
use std::borrow::Cow;
use uuid::Uuid;

use crate::conversation::message::{Message, MessageContentBlock, ProviderMetadata};
use serde_json::{json, Map, Value};
use std::collections::HashMap;

pub const THOUGHT_SIGNATURE_KEY: &str = "thoughtSignature";
const SYNTHETIC_THOUGHT_SIGNATURE: &str = "skip_thought_signature_validator";
const DEFAULT_THINKING_BUDGET: i32 = 8192;

pub fn metadata_with_signature(signature: &str) -> ProviderMetadata {
    let mut map = ProviderMetadata::new();
    map.insert(THOUGHT_SIGNATURE_KEY.to_string(), json!(signature));
    map
}

pub fn get_thought_signature(metadata: &Option<ProviderMetadata>) -> Option<&str> {
    metadata
        .as_ref()
        .and_then(|m| m.get(THOUGHT_SIGNATURE_KEY))
        .and_then(|v| v.as_str())
}

fn is_user_loop_boundary(message: &Message) -> bool {
    message.role == Role::User
        && message
            .content
            .iter()
            .any(|content| !matches!(content, MessageContentBlock::ToolResponse(_)))
}

fn insert_thought_signature(part: &mut Map<String, Value>, signature: &str) {
    part.insert(THOUGHT_SIGNATURE_KEY.to_string(), json!(signature));
}

fn maybe_insert_signature_from_metadata(
    part: &mut Map<String, Value>,
    metadata: &Option<ProviderMetadata>,
) {
    if let Some(signature) = get_thought_signature(metadata) {
        insert_thought_signature(part, signature);
    }
}

fn build_function_response_part(
    id: &str,
    name: &str,
    text: String,
    media: Vec<Value>,
) -> Map<String, Value> {
    let mut part = Map::new();
    let mut function_response = Map::new();
    function_response.insert("id".to_string(), json!(id));
    function_response.insert("name".to_string(), json!(name));
    function_response.insert("response".to_string(), json!({"content": {"text": text}}));
    if !media.is_empty() {
        function_response.insert("parts".to_string(), json!(media));
    }
    part.insert("functionResponse".to_string(), json!(function_response));
    part
}

/// Convert internal Message format to Google's API message specification
pub fn format_messages(messages: &[Message], nested_function_response_media: bool) -> Vec<Value> {
    let filtered: Vec<_> = messages
        .iter()
        .filter(|m| m.is_agent_visible())
        .filter(|message| {
            message.content.iter().any(|content| {
                !matches!(
                    content,
                    MessageContentBlock::ToolConfirmationRequest(_)
                        | MessageContentBlock::ActionRequired(_)
                )
            })
        })
        .collect();

    // Record names as we walk the conversation so a reused tool-call id
    // resolves to the nearest preceding request, not a later overwrite.
    let mut tool_names: HashMap<&str, String> = HashMap::new();

    let active_loop_start_idx = filtered
        .iter()
        .enumerate()
        .rev()
        .find(|(_, m)| is_user_loop_boundary(m))
        .map(|(i, _)| i);

    filtered
        .iter()
        .enumerate()
        .filter_map(|(idx, message)| {
            let role = if message.role == Role::User {
                "user"
            } else {
                "model"
            };
            let include_signature = active_loop_start_idx.is_none_or(|start_idx| idx >= start_idx);
            // Only the first model tool call in a turn is guaranteed to carry
            // a signature for loop continuity.
            let mut needs_synthetic_for_first_model_tool_call =
                include_signature && message.role != Role::User;
            let mut parts = Vec::new();
            for message_content in message.content.iter() {
                match message_content {
                    MessageContentBlock::Text(text) => {
                        if !text.text.is_empty() {
                            parts.push(json!({"text": text.text}));
                        }
                    }
                    MessageContentBlock::ToolRequest(request) => match &request.tool_call {
                        Ok(tool_call) => {
                            let name = sanitize_function_name(&tool_call.name);
                            tool_names.insert(request.id.as_str(), name.clone());
                            let mut function_call_part = Map::new();
                            function_call_part.insert("id".to_string(), json!(request.id));
                            function_call_part.insert("name".to_string(), json!(name));

                            if let Some(args) = &tool_call.arguments {
                                if !args.is_empty() {
                                    function_call_part
                                        .insert("args".to_string(), args.clone().into());
                                }
                            }

                            let mut part = Map::new();
                            part.insert("functionCall".to_string(), json!(function_call_part));

                            if include_signature {
                                if let Some(signature) = get_thought_signature(&request.metadata) {
                                    insert_thought_signature(&mut part, signature);
                                } else if needs_synthetic_for_first_model_tool_call {
                                    insert_thought_signature(
                                        &mut part,
                                        SYNTHETIC_THOUGHT_SIGNATURE,
                                    );
                                }
                            }
                            needs_synthetic_for_first_model_tool_call = false;

                            parts.push(json!(part));
                        }
                        Err(e) => {
                            parts.push(json!({"text":format!("Error: {}", e)}));
                        }
                    },
                    MessageContentBlock::ToolResponse(response) => match &response.tool_result {
                        Ok(result) => {
                            let mut tool_content = Vec::new();
                            let mut media = Vec::new();
                            for content in result.content.iter().cloned() {
                                let inline = match &content {
                                    ContentBlock::Image(image) => {
                                        Some((image.mime_type.clone(), image.data.clone()))
                                    }
                                    ContentBlock::Resource(embedded) => match &embedded.resource {
                                        ResourceContents::BlobResourceContents {
                                            blob,
                                            mime_type,
                                            ..
                                        } => mime_type
                                            .clone()
                                            .filter(|m| !m.is_empty())
                                            .map(|mime| (mime, blob.clone())),
                                        _ => None,
                                    },
                                    _ => None,
                                };
                                match inline {
                                    Some((mime, data)) if nested_function_response_media => {
                                        media.push(json!({
                                            "inlineData": {"mimeType": mime, "data": data}
                                        }));
                                    }
                                    Some((mime, data)) => {
                                        parts.push(json!({
                                            "inline_data": {"mime_type": mime, "data": data}
                                        }));
                                    }
                                    None => tool_content.push(content),
                                }
                            }
                            let mut text = tool_content
                                .iter()
                                .filter_map(|c| match c {
                                    ContentBlock::Text(t) => Some(t.text.clone()),
                                    ContentBlock::Resource(raw_embedded_resource) => Some(
                                        extract_text_from_resource(&raw_embedded_resource.resource),
                                    ),
                                    _ => None,
                                })
                                .collect::<Vec<_>>()
                                .join("\n");

                            if text.is_empty() {
                                text = "Tool call is done.".to_string();
                            }
                            let name = tool_names
                                .get(response.id.as_str())
                                .map(String::as_str)
                                .unwrap_or(response.id.as_str());
                            let mut part =
                                build_function_response_part(&response.id, name, text, media);
                            if include_signature {
                                maybe_insert_signature_from_metadata(&mut part, &response.metadata);
                            }
                            parts.push(json!(part));
                        }
                        Err(e) => {
                            let name = tool_names
                                .get(response.id.as_str())
                                .map(String::as_str)
                                .unwrap_or(response.id.as_str());
                            let mut part = build_function_response_part(
                                &response.id,
                                name,
                                format!("Error: {}", e),
                                Vec::new(),
                            );
                            if include_signature {
                                maybe_insert_signature_from_metadata(&mut part, &response.metadata);
                            }
                            parts.push(json!(part));
                        }
                    },
                    MessageContentBlock::Thinking(_) => {}
                    MessageContentBlock::Image(image) => {
                        parts.push(json!({
                            "inline_data": {
                                "mime_type": image.mime_type,
                                "data": image.data,
                            }
                        }));
                    }
                    MessageContentBlock::Document(document) => {
                        if document_media_type_is_supported(&document.mime_type) {
                            parts.push(json!({
                                "inline_data": {
                                    "mime_type": document.mime_type,
                                    "data": document.data,
                                }
                            }));
                        } else {
                            parts.push(json!({
                                "text": unsupported_document_text(document, UNSUPPORTED_MEDIA_TYPE_REASON)
                            }));
                        }
                    }

                    _ => {}
                }
            }
            if parts.is_empty() {
                None
            } else {
                Some(json!({"role": role, "parts": parts}))
            }
        })
        .collect()
}

pub fn format_tools(tools: &[Tool]) -> Vec<Value> {
    tools
        .iter()
        .map(|tool| {
            let mut parameters = Map::new();
            parameters.insert("name".to_string(), json!(tool.name));
            parameters.insert("description".to_string(), json!(tool.description));

            // Use parametersJsonSchema which supports full JSON Schema including $ref/$defs
            if tool
                .input_schema
                .get("properties")
                .and_then(|v| v.as_object())
                .is_some_and(|p| !p.is_empty())
            {
                parameters.insert("parametersJsonSchema".to_string(), json!(tool.input_schema));
            }
            json!(parameters)
        })
        .collect()
}

fn process_response_part_impl(
    part: &Value,
    last_signature: &mut Option<String>,
) -> Option<MessageContentBlock> {
    let signature = part.get(THOUGHT_SIGNATURE_KEY).and_then(|v| v.as_str());
    let is_thought = part
        .get("thought")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);

    if let Some(sig) = signature {
        *last_signature = Some(sig.to_string());
    }

    let text_value = part.get("text");
    if let Some(text) = text_value.and_then(|v| v.as_str()) {
        if text.is_empty() {
            return None;
        }
        if is_thought {
            match signature {
                Some(sig) => Some(MessageContentBlock::thinking(
                    text.to_string(),
                    sig.to_string(),
                )),
                None => Some(MessageContentBlock::thinking(text.to_string(), "")),
            }
        } else {
            Some(MessageContentBlock::text(text.to_string()))
        }
    } else if text_value.is_some() {
        tracing::warn!(
            "Google response part has 'text' field but it's not a string: {:?}",
            text_value
        );
        None
    } else if let Some(function_call) = part.get("functionCall") {
        let id = function_call
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let name = function_call["name"].as_str().unwrap_or_default();

        if !is_valid_function_name(name) {
            let error = ErrorData {
                code: ErrorCode::INVALID_REQUEST,
                message: Cow::from(format!(
                    "The provided function name '{}' had invalid characters, it must match this regex [a-zA-Z0-9_-]+",
                    name
                )),
                data: None,
            };
            Some(MessageContentBlock::tool_request(id, Err(error)))
        } else {
            let arguments = function_call
                .get("args")
                .map(|params| object(params.clone()));
            let effective_signature = signature.or(last_signature.as_deref());
            let metadata = effective_signature.map(metadata_with_signature);

            Some(MessageContentBlock::tool_request_with_metadata(
                id,
                Ok({
                    let mut params = CallToolRequestParams::new(name.to_string());
                    if let Some(args) = arguments {
                        params = params.with_arguments(args);
                    }
                    params
                }),
                metadata.as_ref(),
            ))
        }
    } else {
        None
    }
}

pub fn response_to_message(response: Value) -> Result<Message> {
    let role = Role::Assistant;
    let created = chrono::Utc::now().timestamp();

    let parts = response
        .get("candidates")
        .and_then(|v| v.as_array())
        .and_then(|c| c.first())
        .and_then(|c| c.get("content"))
        .and_then(|c| c.get("parts"))
        .and_then(|p| p.as_array());

    let Some(parts) = parts else {
        return Ok(Message::new(role, created, Vec::new()));
    };

    let mut content = Vec::new();
    let mut last_signature: Option<String> = None;

    for part in parts {
        if let Some(msg_content) = process_response_part_impl(part, &mut last_signature) {
            content.push(msg_content);
        }
    }
    Ok(Message::new(role, created, content))
}

/// Extract usage information from Google's API response
pub fn get_usage(data: &Value) -> Result<Usage> {
    if let Some(usage_meta_data) = data.get("usageMetadata") {
        let input_tokens = usage_meta_data
            .get("promptTokenCount")
            .and_then(|v| v.as_u64())
            .map(|v| v as i32);
        // `candidatesTokenCount` is the visible output; thinking models
        // (Gemini 2.5/3) report reasoning tokens separately in
        // `thoughtsTokenCount`, and per the API spec `totalTokenCount` =
        // prompt + thoughts + candidates. Fold thoughts into `output_tokens` so
        // the record reconciles (input + output == total) and cost, which
        // Google bills at the output rate, is correct -- matching the OpenAI
        // (completion_tokens includes reasoning) and Anthropic (output_tokens
        // includes thinking) adapters.
        let candidates_tokens = usage_meta_data
            .get("candidatesTokenCount")
            .and_then(|v| v.as_u64());
        let thoughts_tokens = usage_meta_data
            .get("thoughtsTokenCount")
            .and_then(|v| v.as_u64());
        let output_tokens = match (candidates_tokens, thoughts_tokens) {
            (None, None) => None,
            (candidates, thoughts) => {
                Some((candidates.unwrap_or(0) + thoughts.unwrap_or(0)) as i32)
            }
        };
        let total_tokens = usage_meta_data
            .get("totalTokenCount")
            .and_then(|v| v.as_u64())
            .map(|v| v as i32);
        // promptTokenCount already includes cachedContentTokenCount
        let cached_tokens = usage_meta_data
            .get("cachedContentTokenCount")
            .and_then(|v| v.as_u64())
            .map(|v| v as i32);
        Ok(Usage::new(input_tokens, output_tokens, total_tokens)
            .with_cache_tokens(cached_tokens, None))
    } else {
        tracing::debug!(
            "Failed to get usage data: {}",
            ProviderError::UsageError("No usage data found in response".to_string())
        );
        // If no usage data, return None for all values
        Ok(Usage::new(None, None, None))
    }
}

pub fn response_to_streaming_message<S>(
    mut stream: S,
) -> impl futures::Stream<Item = anyhow::Result<(Option<Message>, Option<ProviderUsage>)>> + 'static
where
    S: futures::Stream<Item = anyhow::Result<String>> + Unpin + Send + 'static,
{
    use async_stream::try_stream;
    use futures::StreamExt;

    try_stream! {
        let mut final_usage: Option<ProviderUsage> = None;
        let mut last_signature: Option<String> = None;
        let stream_id = Uuid::new_v4().to_string();
        let mut incomplete_data: Option<String> = None;
        let mut last_finish_reason: Option<String> = None;
        let mut last_response_id: Option<String> = None;

        while let Some(line_result) = stream.next().await {
            let line = line_result?;

            if line.trim().is_empty() {
                continue;
            }

            let data_part = if line.starts_with("data: ") {
                line.strip_prefix("data: ").unwrap()
            } else if line.starts_with("event:") || line.starts_with("id:") || line.starts_with("retry:") {
                continue;
            } else if incomplete_data.is_some() {
                &line
            } else {
                continue;
            };

            if data_part.trim() == "[DONE]" {
                break;
            }

            let chunk: Value = if let Some(ref mut incomplete) = incomplete_data {
                incomplete.push_str(data_part);
                match serde_json::from_str(incomplete) {
                    Ok(v) => {
                        incomplete_data = None;
                        v
                    }
                    Err(e) => {
                        if e.is_eof() {
                            continue;
                        }
                        tracing::warn!("Failed to parse streaming chunk: {}", e);
                        incomplete_data = None;
                        continue;
                    }
                }
            } else {
                match serde_json::from_str(data_part) {
                    Ok(v) => v,
                    Err(e) => {
                        if e.is_eof() {
                            incomplete_data = Some(data_part.to_string());
                            continue;
                        }
                        tracing::warn!("Failed to parse streaming chunk: {}", e);
                        continue;
                    }
                }
            };

            if let Some(error) = chunk.get("error") {
                let message = error
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("Unknown error");
                let status = error
                    .get("status")
                    .and_then(|s| s.as_str())
                    .unwrap_or("UNKNOWN");
                Err::<(), ProviderError>(ProviderError::RequestFailed(format!(
                    "Google API error ({status}): {message}"
                )))?;
            }

            if let Ok(usage) = get_usage(&chunk) {
                if usage.input_tokens.is_some() || usage.output_tokens.is_some() {
                    let model = chunk.get("modelVersion")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown")
                        .to_string();
                    final_usage = Some(ProviderUsage::new(model, usage));
                }
            }

            if let Some(response_id) = chunk.get("responseId").and_then(|v| v.as_str()) {
                last_response_id = Some(response_id.to_string());
            }

            let candidate = chunk
                .get("candidates")
                .and_then(|v| v.as_array())
                .and_then(|c| c.first());
            if let Some(reason) = candidate
                .and_then(|c| c.get("finishReason"))
                .and_then(|v| v.as_str())
            {
                last_finish_reason = Some(reason.to_string());
            }

            let parts = candidate
                .and_then(|c| c.get("content"))
                .and_then(|c| c.get("parts"))
                .and_then(|p| p.as_array());

            if let Some(parts) = parts {
                for part in parts {
                    if let Some(content) = process_response_part_impl(part, &mut last_signature) {
                        let message = Message::new(
                            Role::Assistant,
                            chrono::Utc::now().timestamp(),
                            vec![content],
                        ).with_id(stream_id.clone());
                        yield (Some(message), None);
                    }
                }
            }
        }

        if let Some(mut usage) = final_usage {
            if let Some(reason) = last_finish_reason {
                usage.finish_reasons = Some(vec![reason]);
            }
            usage.response_id = last_response_id;
            yield (None, Some(usage));
        }
    }
}

#[derive(Serialize)]
struct TextPart<'a> {
    text: &'a str,
}

#[derive(Serialize)]
struct SystemInstruction<'a> {
    parts: [TextPart<'a>; 1],
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ToolsWrapper {
    function_declarations: Vec<Value>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking_config: Option<ThinkingConfig>,
}

#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
enum ThinkingLevel {
    Minimal,
    Low,
    Medium,
    High,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ThinkingConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking_level: Option<ThinkingLevel>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking_budget: Option<i32>,
    include_thoughts: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GoogleRequest<'a> {
    system_instruction: SystemInstruction<'a>,
    contents: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<ToolsWrapper>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generation_config: Option<GenerationConfig>,
}

fn get_thinking_config(
    model_config: &ModelConfig,
    thinking_budget: Option<i32>,
) -> Option<ThinkingConfig> {
    if model_config.reasoning == Some(false)
        || model_config.thinking_effort() == Some(ThinkingEffort::Off)
    {
        let model_name = model_config.model_name.to_lowercase();
        if model_name.starts_with("gemini-3.5") || model_name.starts_with("gemini-3.6") {
            return Some(ThinkingConfig {
                thinking_level: Some(ThinkingLevel::Minimal),
                thinking_budget: None,
                include_thoughts: false,
            });
        }
        // Gemini 2.5 Flash defaults to dynamic thinking; only an explicit budget
        // of 0 turns it off. Other families can't be disabled, so leave them unset.
        if model_config
            .model_name
            .to_lowercase()
            .starts_with("gemini-2.5-flash")
        {
            return Some(ThinkingConfig {
                thinking_level: None,
                thinking_budget: Some(0),
                include_thoughts: false,
            });
        }
        return None;
    }
    let model_name = model_config.model_name.to_lowercase();
    let is_gemini_3 = model_name.starts_with("gemini-3");
    let is_gemini_25 = model_name.starts_with("gemini-2.5");
    if !is_gemini_3 && !is_gemini_25 {
        return None;
    }

    if is_gemini_3 {
        let effort = model_config
            .thinking_effort()
            .unwrap_or(ThinkingEffort::Off);
        if effort == ThinkingEffort::Off {
            return None;
        }
        let thinking_level = match effort {
            ThinkingEffort::Off | ThinkingEffort::Low => ThinkingLevel::Low,
            ThinkingEffort::Medium if model_name.starts_with("gemini-3-pro") => ThinkingLevel::Low,
            ThinkingEffort::Medium => ThinkingLevel::Medium,
            ThinkingEffort::High | ThinkingEffort::Max => ThinkingLevel::High,
        };

        Some(ThinkingConfig {
            thinking_level: Some(thinking_level),
            thinking_budget: None,
            include_thoughts: true,
        })
    } else {
        let thinking_budget = match model_config
            .request_param::<i32>("thinking_budget")
            .or(thinking_budget)
        {
            Some(budget) if budget >= 0 => budget,
            Some(budget) => {
                tracing::warn!(
                    "Invalid thinking budget '{}' for model '{}'. Must be >= 0. Using '{}'.",
                    budget,
                    model_config.model_name,
                    DEFAULT_THINKING_BUDGET,
                );
                DEFAULT_THINKING_BUDGET
            }
            None => DEFAULT_THINKING_BUDGET,
        };
        Some(ThinkingConfig {
            thinking_level: None,
            thinking_budget: Some(thinking_budget),
            include_thoughts: true,
        })
    }
}

pub fn create_request(
    model_config: &ModelConfig,
    system: &str,
    messages: &[Message],
    tools: &[Tool],
) -> Result<Value> {
    create_request_impl(model_config, system, messages, tools, None)
}

pub fn create_request_with_thinking_budget(
    model_config: &ModelConfig,
    system: &str,
    messages: &[Message],
    tools: &[Tool],
    thinking_budget: Option<i32>,
) -> Result<Value> {
    create_request_impl(model_config, system, messages, tools, thinking_budget)
}

fn create_request_impl(
    model_config: &ModelConfig,
    system: &str,
    messages: &[Message],
    tools: &[Tool],
    thinking_budget: Option<i32>,
) -> Result<Value> {
    let tools_wrapper = if tools.is_empty() {
        None
    } else {
        Some(ToolsWrapper {
            function_declarations: format_tools(tools),
        })
    };

    let thinking_config = get_thinking_config(model_config, thinking_budget);
    let temperature = (!model_config
        .model_name
        .to_lowercase()
        .starts_with("gemini-3"))
    .then(|| model_config.temperature.map(|t| t as f64))
    .flatten();

    let generation_config = Some(GenerationConfig {
        temperature,
        max_output_tokens: Some(model_config.max_output_tokens()),
        thinking_config,
    });

    let request = GoogleRequest {
        system_instruction: SystemInstruction {
            parts: [TextPart { text: system }],
        },
        contents: format_messages(
            messages,
            model_config
                .model_name
                .to_lowercase()
                .starts_with("gemini-3"),
        ),
        tools: tools_wrapper,
        generation_config,
    };

    Ok(serde_json::to_value(request)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::message::Message;
    use rmcp::model::{CallToolRequestParams, CallToolResult};
    use rmcp::{model::ContentBlock, object};
    use serde_json::json;
    use std::collections::HashMap;

    fn set_up_text_message(text: &str, role: Role) -> Message {
        Message::new(role, 0, vec![MessageContentBlock::text(text.to_string())])
    }

    fn set_up_tool_request_message(id: &str, tool_call: CallToolRequestParams) -> Message {
        Message::new(
            Role::User,
            0,
            vec![MessageContentBlock::tool_request(
                id.to_string(),
                Ok(tool_call),
            )],
        )
    }

    fn set_up_action_required_message(id: &str, tool_call: CallToolRequestParams) -> Message {
        Message::new(
            Role::User,
            0,
            vec![MessageContentBlock::action_required(
                id.to_string(),
                tool_call.name.to_string().clone(),
                tool_call.arguments.unwrap_or_default().clone(),
                Some("goose would like to call the above tool. Allow? (y/n):".to_string()),
            )],
        )
    }

    fn set_up_tool_response_message(id: &str, tool_response: Vec<ContentBlock>) -> Message {
        Message::new(
            Role::Assistant,
            0,
            vec![MessageContentBlock::tool_response(
                id.to_string(),
                Ok(CallToolResult::success(tool_response)),
            )],
        )
    }

    #[test]
    fn test_get_usage() {
        let data = json!({
            "usageMetadata": {
                "promptTokenCount": 1,
                "candidatesTokenCount": 2,
                "totalTokenCount": 3
            }
        });
        let usage = get_usage(&data).unwrap();
        assert_eq!(usage.input_tokens, Some(1));
        assert_eq!(usage.output_tokens, Some(2));
        assert_eq!(usage.total_tokens, Some(3));
        assert_eq!(usage.cache_read_input_tokens, None);
        assert_eq!(usage.cache_write_input_tokens, None);
    }

    #[test]
    fn test_get_usage_with_cached_content() {
        let data = json!({
            "usageMetadata": {
                "promptTokenCount": 100,
                "candidatesTokenCount": 20,
                "totalTokenCount": 120,
                "cachedContentTokenCount": 80
            }
        });
        let usage = get_usage(&data).unwrap();
        assert_eq!(usage.input_tokens, Some(100));
        assert_eq!(usage.output_tokens, Some(20));
        assert_eq!(usage.total_tokens, Some(120));
        assert_eq!(usage.cache_read_input_tokens, Some(80));
        assert_eq!(usage.cache_write_input_tokens, None);
    }

    #[test]
    fn test_get_usage_includes_thinking_tokens() {
        // Gemini thinking models (2.5/3, the goose defaults) report reasoning
        // tokens separately in `thoughtsTokenCount`, and per the API spec
        // `totalTokenCount` = prompt + thoughts + candidates. `output_tokens`
        // must include thoughts so the record reconciles (input + output ==
        // total) and cost, which Google bills at the output rate, is correct --
        // matching the OpenAI (completion_tokens includes reasoning) and
        // Anthropic (output_tokens includes thinking) adapters.
        let data = json!({
            "usageMetadata": {
                "promptTokenCount": 100,
                "candidatesTokenCount": 50,
                "thoughtsTokenCount": 200,
                "totalTokenCount": 350
            }
        });
        let usage = get_usage(&data).unwrap();
        assert_eq!(usage.input_tokens, Some(100));
        assert_eq!(usage.output_tokens, Some(250));
        assert_eq!(usage.total_tokens, Some(350));
        assert_eq!(
            usage.input_tokens.unwrap() + usage.output_tokens.unwrap(),
            usage.total_tokens.unwrap(),
        );
    }

    #[test]
    fn test_message_to_google_spec_text_message() {
        let messages = vec![
            set_up_text_message("Hello", Role::User),
            set_up_text_message("World", Role::Assistant),
        ];
        let payload = format_messages(&messages, false);
        assert_eq!(payload.len(), 2);
        assert_eq!(payload[0]["role"], "user");
        assert_eq!(payload[0]["parts"][0]["text"], "Hello");
        assert_eq!(payload[1]["role"], "model");
        assert_eq!(payload[1]["parts"][0]["text"], "World");
    }

    #[test]
    fn test_message_to_google_spec_image_message() {
        use rmcp::model::ImageContent;

        let image = ImageContent::new("base64encodeddata", "image/png");
        let messages = vec![Message::new(
            Role::User,
            0,
            vec![
                MessageContentBlock::text("What is in this image?".to_string()),
                MessageContentBlock::Image(image),
            ],
        )];
        let payload = format_messages(&messages, false);

        assert_eq!(payload.len(), 1);
        assert_eq!(payload[0]["role"], "user");
        assert_eq!(payload[0]["parts"][0]["text"], "What is in this image?");
        assert_eq!(
            payload[0]["parts"][1]["inline_data"]["mime_type"],
            "image/png"
        );
        assert_eq!(
            payload[0]["parts"][1]["inline_data"]["data"],
            "base64encodeddata"
        );
    }

    #[test]
    fn test_message_to_google_spec_document_only_message() {
        let messages = vec![Message::new(
            Role::User,
            0,
            vec![MessageContentBlock::document(
                "base64pdfdata".to_string(),
                "application/pdf".to_string(),
                Some("report.pdf".to_string()),
            )],
        )];
        let payload = format_messages(&messages, false);

        assert_eq!(payload.len(), 1);
        assert_eq!(payload[0]["role"], "user");
        assert_eq!(
            payload[0]["parts"][0]["inline_data"]["mime_type"],
            "application/pdf"
        );
        assert_eq!(
            payload[0]["parts"][0]["inline_data"]["data"],
            "base64pdfdata"
        );
    }

    #[test]
    fn test_message_to_google_spec_tool_request_message() {
        let arguments = json!({
            "param1": "value1"
        });
        let messages = vec![
            set_up_tool_request_message(
                "id",
                CallToolRequestParams::new("tool_name").with_arguments(object(arguments.clone())),
            ),
            set_up_action_required_message(
                "id2",
                CallToolRequestParams::new("tool_name_2").with_arguments(object(arguments.clone())),
            ),
        ];
        let payload = format_messages(&messages, false);
        assert_eq!(payload.len(), 1);
        assert_eq!(payload[0]["role"], "user");
        assert_eq!(payload[0]["parts"][0]["functionCall"]["id"], "id");
        assert_eq!(payload[0]["parts"][0]["functionCall"]["args"], arguments);
    }

    #[test]
    fn test_message_to_google_spec_tool_result_message() {
        let tool_result: Vec<ContentBlock> = vec![ContentBlock::text("Hello")];
        let messages = vec![set_up_tool_response_message("response_id", tool_result)];
        let payload = format_messages(&messages, false);
        assert_eq!(payload.len(), 1);
        assert_eq!(payload[0]["role"], "model");
        assert_eq!(
            payload[0]["parts"][0]["functionResponse"]["name"],
            "response_id"
        );
        assert_eq!(
            payload[0]["parts"][0]["functionResponse"]["id"],
            "response_id"
        );
        assert_eq!(
            payload[0]["parts"][0]["functionResponse"]["response"]["content"]["text"],
            "Hello"
        );
    }

    #[test]
    fn test_message_to_google_spec_sanitizes_resource_tool_response() {
        let messages = vec![set_up_tool_response_message(
            "response_id",
            vec![ContentBlock::embedded_text(
                "file:///result.txt",
                "visible\u{E0041}text",
            )],
        )];

        let payload = format_messages(&messages, false);

        assert_eq!(
            payload[0]["parts"][0]["functionResponse"]["response"]["content"]["text"],
            "visibletext"
        );
    }

    #[test]
    fn test_function_response_matches_function_call() {
        let messages = vec![
            set_up_tool_request_message("call_123", CallToolRequestParams::new("read_file")),
            set_up_tool_response_message("call_123", vec![ContentBlock::text("contents")]),
        ];

        let payload = format_messages(&messages, false);

        assert_eq!(
            payload[1]["parts"][0]["functionResponse"],
            json!({
                "id": "call_123",
                "name": "read_file",
                "response": {"content": {"text": "contents"}}
            })
        );
    }

    #[test]
    fn test_reused_tool_call_id_keeps_each_response_name() {
        let messages = vec![
            set_up_tool_request_message(
                "call_3407",
                CallToolRequestParams::new("download_document"),
            ),
            set_up_tool_response_message("call_3407", vec![ContentBlock::text("the document")]),
            set_up_tool_request_message("call_3407", CallToolRequestParams::new("shell")),
            set_up_tool_response_message("call_3407", vec![ContentBlock::text("shell output")]),
        ];

        let payload = format_messages(&messages, false);

        assert_eq!(
            payload[1]["parts"][0]["functionResponse"],
            json!({
                "id": "call_3407",
                "name": "download_document",
                "response": {"content": {"text": "the document"}}
            })
        );
        assert_eq!(
            payload[3]["parts"][0]["functionResponse"],
            json!({
                "id": "call_3407",
                "name": "shell",
                "response": {"content": {"text": "shell output"}}
            })
        );
    }

    #[test]
    fn test_image_tool_result_is_nested_in_function_response() {
        let messages = vec![
            set_up_tool_request_message("call_123", CallToolRequestParams::new("screenshot")),
            set_up_tool_response_message(
                "call_123",
                vec![
                    ContentBlock::text("Screenshot captured"),
                    ContentBlock::image("base64encodeddata", "image/png"),
                ],
            ),
        ];

        let payload = format_messages(&messages, true);

        assert_eq!(payload[1]["parts"].as_array().unwrap().len(), 1);
        assert_eq!(
            payload[1]["parts"][0]["functionResponse"],
            json!({
                "id": "call_123",
                "name": "screenshot",
                "response": {"content": {"text": "Screenshot captured"}},
                "parts": [{
                    "inlineData": {
                        "mimeType": "image/png",
                        "data": "base64encodeddata"
                    }
                }]
            })
        );
    }

    #[test]
    fn test_blob_resource_tool_result_is_forwarded_as_media() {
        let blob = ContentBlock::resource(ResourceContents::BlobResourceContents {
            uri: "file:///shot.png".to_string(),
            mime_type: Some("image/png".to_string()),
            blob: "aGVsbG8=".to_string(),
            meta: None,
        });
        let messages = vec![
            set_up_tool_request_message("call_123", CallToolRequestParams::new("screenshot")),
            set_up_tool_response_message("call_123", vec![blob]),
        ];

        let payload = format_messages(&messages, true);

        assert_eq!(
            payload[1]["parts"][0]["functionResponse"]["parts"],
            json!([{"inlineData": {"mimeType": "image/png", "data": "aGVsbG8="}}])
        );
    }

    #[test]
    fn test_message_to_google_spec_tool_result_multiple_texts() {
        let tool_result: Vec<ContentBlock> = vec![
            ContentBlock::text("Hello"),
            ContentBlock::text("World"),
            ContentBlock::embedded_text("test_uri", "This is a test."),
        ];

        let messages = vec![set_up_tool_response_message("response_id", tool_result)];
        let payload = format_messages(&messages, false);

        let expected_payload = vec![json!({
            "role": "model",
            "parts": [
                {
                    "functionResponse": {
                        "id": "response_id",
                        "name": "response_id",
                        "response": {
                            "content": {
                                "text": "Hello\nWorld\nThis is a test."
                            }
                        }
                    }
                }
            ]
        })];

        assert_eq!(payload, expected_payload);
    }

    #[test]
    fn test_tools_to_google_spec_with_valid_tools() {
        let params = object!({
            "properties": {
                "param1": {
                    "type": "string",
                    "description": "A parameter"
                }
            }
        });
        let tools = vec![Tool::new("tool1", "description1", params.clone())];
        let result = format_tools(&tools);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["name"], "tool1");
        assert_eq!(result[0]["description"], "description1");
        assert!(result[0].get("parametersJsonSchema").is_some());
        assert!(result[0].get("parameters").is_none());
        assert_eq!(result[0]["parametersJsonSchema"], json!(params));
    }

    #[test]
    fn test_tools_to_google_spec_with_empty_properties() {
        let tools = vec![Tool::new(
            "tool1".to_string(),
            "description1".to_string(),
            object!({
                "properties": {}
            }),
        )];
        let result = format_tools(&tools);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["name"], "tool1");
        assert_eq!(result[0]["description"], "description1");
        assert!(result[0].get("parametersJsonSchema").is_none());
    }

    #[test]
    fn test_response_to_message_with_no_candidates() {
        let response = json!({});
        let message = response_to_message(response).unwrap();
        assert_eq!(message.role, Role::Assistant);
        assert!(message.content.is_empty());
    }

    #[test]
    fn test_response_to_message_with_text_part() {
        let response = json!({
            "candidates": [{
                "content": {
                    "parts": [{
                        "text": "Hello, world!"
                    }]
                }
            }]
        });
        let message = response_to_message(response).unwrap();
        assert_eq!(message.role, Role::Assistant);
        assert_eq!(message.content.len(), 1);
        if let MessageContentBlock::Text(text) = &message.content[0] {
            assert_eq!(text.text, "Hello, world!");
        } else {
            panic!("Expected text content");
        }
    }

    #[test]
    fn test_response_to_message_with_invalid_function_name() {
        let response = json!({
            "candidates": [{
                "content": {
                    "parts": [{
                        "functionCall": {
                            "name": "invalid name!",
                            "args": {}
                        }
                    }]
                }
            }]
        });
        let message = response_to_message(response).unwrap();
        assert_eq!(message.role, Role::Assistant);
        assert_eq!(message.content.len(), 1);
        if let Err(error) = &message.content[0].as_tool_request().unwrap().tool_call {
            assert!(matches!(
                error,
                ErrorData {
                    code: ErrorCode::INVALID_REQUEST,
                    message: _,
                    data: None,
                }
            ));
        } else {
            panic!("Expected tool request error");
        }
    }

    #[test]
    fn test_response_to_message_with_valid_function_call() {
        let response = json!({
            "candidates": [{
                "content": {
                    "parts": [{
                        "functionCall": {
                            "id": "call_123",
                            "name": "valid_name",
                            "args": {
                                "param": "value"
                            }
                        }
                    }]
                }
            }]
        });
        let message = response_to_message(response).unwrap();
        assert_eq!(message.role, Role::Assistant);
        assert_eq!(message.content.len(), 1);
        assert_eq!(message.content[0].as_tool_request().unwrap().id, "call_123");
        if let Ok(tool_call) = &message.content[0].as_tool_request().unwrap().tool_call {
            assert_eq!(tool_call.name, "valid_name");
            assert_eq!(
                tool_call
                    .arguments
                    .as_ref()
                    .and_then(|args| args.get("param"))
                    .and_then(|v| v.as_str()),
                Some("value")
            );
        } else {
            panic!("Expected valid tool request");
        }
    }

    #[test]
    fn test_response_to_message_with_empty_content() {
        let tool_result: Vec<ContentBlock> = Vec::new();

        let messages = vec![set_up_tool_response_message("response_id", tool_result)];
        let payload = format_messages(&messages, false);

        let expected_payload = vec![json!({
            "role": "model",
            "parts": [
                {
                    "functionResponse": {
                        "id": "response_id",
                        "name": "response_id",
                        "response": {
                            "content": {
                                "text": "Tool call is done."
                            }
                        }
                    }
                }
            ]
        })];

        assert_eq!(payload, expected_payload);
    }

    #[test]
    fn test_tools_uses_parameters_json_schema() {
        let params = object!({
            "properties": {
                "field": {
                    "type": ["string", "null"],
                    "description": "A field"
                }
            }
        });
        let tools = vec![Tool::new("test_tool", "test description", params.clone())];
        let result = format_tools(&tools);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["name"], "test_tool");
        assert!(result[0].get("parametersJsonSchema").is_some());
        assert_eq!(result[0]["parametersJsonSchema"], json!(params));
    }

    fn google_response(parts: Vec<Value>) -> Value {
        json!({"candidates": [{"content": {"role": "model", "parts": parts}}]})
    }

    fn tool_result(text: &str) -> CallToolResult {
        CallToolResult::success(vec![ContentBlock::text(text)])
    }

    #[test]
    fn test_thought_signature_roundtrip() {
        const SIG: &str = "thought_sig_abc";

        let response_with_tools = google_response(vec![
            json!({"text": "Let me think...", "thought": true, "thoughtSignature": SIG}),
            json!({"functionCall": {"name": "shell", "args": {"cmd": "ls"}}, "thoughtSignature": SIG}),
            json!({"functionCall": {"name": "read", "args": {}}}),
        ]);

        let native = response_to_message(response_with_tools).unwrap();
        assert_eq!(native.content.len(), 3, "Expected thinking + 2 tool calls");

        let thinking = native.content[0]
            .as_thinking()
            .expect("Text with function calls should be Thinking");
        assert_eq!(thinking.signature, SIG);

        let req1 = native.content[1]
            .as_tool_request()
            .expect("Second part should be ToolRequest");
        let req2 = native.content[2]
            .as_tool_request()
            .expect("Third part should be ToolRequest");
        assert_eq!(get_thought_signature(&req1.metadata), Some(SIG));
        assert_eq!(
            get_thought_signature(&req2.metadata),
            Some(SIG),
            "Should inherit"
        );

        let mut tool_response = Message::user();
        tool_response.add_tool_response_with_metadata(
            req1.id.clone(),
            Ok(tool_result("output")),
            req1.metadata.as_ref(),
        );
        let user_prompt = set_up_text_message("List files", Role::User);
        let google_out = format_messages(
            &[user_prompt.clone(), native.clone(), tool_response.clone()],
            false,
        );
        assert_eq!(google_out[1]["parts"][0]["thoughtSignature"], SIG);
        assert_eq!(google_out[2]["parts"][0]["thoughtSignature"], SIG);

        let second_assistant = response_to_message(google_response(vec![json!({
            "functionCall": {"name": "echo", "args": {}},
            "thoughtSignature": "sig_456"
        })]))
        .unwrap();
        let google_multi = format_messages(
            &[user_prompt, native, tool_response, second_assistant],
            false,
        );
        assert_eq!(google_multi[1]["parts"][0]["thoughtSignature"], SIG);
        assert_eq!(google_multi[2]["parts"][0]["thoughtSignature"], SIG);
        assert_eq!(google_multi[3]["parts"][0]["thoughtSignature"], "sig_456");

        let final_response_with_sig =
            google_response(vec![json!({"text": "Done!", "thoughtSignature": SIG})]);
        let final_native_with_sig = response_to_message(final_response_with_sig).unwrap();
        assert!(
            final_native_with_sig.content[0].as_text().is_some(),
            "Text with signature but no function calls should be regular text (final response)"
        );

        let final_response_no_sig = google_response(vec![json!({"text": "Done!"})]);
        let final_native_no_sig = response_to_message(final_response_no_sig).unwrap();
        assert!(
            final_native_no_sig.content[0].as_text().is_some(),
            "Text without signature is regular text"
        );
    }

    #[test]
    fn test_thought_without_signature_maps_to_thinking() {
        let response = google_response(vec![json!({
            "text": "Working through options...",
            "thought": true
        })]);
        let native = response_to_message(response).unwrap();
        assert_eq!(native.content.len(), 1);
        assert!(native.content[0].as_thinking().is_some());
    }

    #[test]
    fn test_format_messages_omits_messages_with_empty_parts() {
        let user_prompt = set_up_text_message("hello", Role::User);
        let thinking_only =
            Message::assistant().with_thinking("internal".to_string(), "sig_123".to_string());
        let reasoning_only = response_to_message(google_response(vec![json!({
            "text": "deliberating",
            "thought": true
        })]))
        .unwrap();

        let formatted = format_messages(&[user_prompt, thinking_only, reasoning_only], false);
        assert_eq!(formatted.len(), 1);
        assert_eq!(formatted[0]["role"], "user");
        assert_eq!(formatted[0]["parts"][0]["text"], "hello");
    }

    #[test]
    fn test_active_loop_injects_synthetic_signature_for_first_model_tool_call() {
        let user_prompt = set_up_text_message("Find a restaurant", Role::User);
        let assistant_tool = response_to_message(google_response(vec![json!({
            "functionCall": {"name": "find_restaurant", "args": {"cuisine": "italian"}}
        })]))
        .unwrap();

        let formatted = format_messages(&[user_prompt, assistant_tool], false);
        assert_eq!(
            formatted[1]["parts"][0][THOUGHT_SIGNATURE_KEY],
            SYNTHETIC_THOUGHT_SIGNATURE
        );
    }

    const GOOGLE_TEXT_STREAM: &str = concat!(
        r#"data: {"candidates": [{"content": {"role": "model", "#,
        r#""parts": [{"text": "Hello"}]}}]}"#,
        "\n",
        r#"data: {"candidates": [{"content": {"role": "model", "#,
        r#""parts": [{"text": " world"}]}}]}"#,
        "\n",
        r#"data: {"candidates": [{"content": {"role": "model", "#,
        r#""parts": [{"text": "!"}]}}], "#,
        r#""usageMetadata": {"promptTokenCount": 10, "#,
        r#""candidatesTokenCount": 3, "totalTokenCount": 13}}"#
    );

    const GOOGLE_FUNCTION_STREAM: &str = concat!(
        r#"data: {"candidates": [{"content": {"role": "model", "#,
        r#""parts": [{"functionCall": {"name": "test_tool", "#,
        r#""args": {"param": "value"}}}]}}], "#,
        r#""usageMetadata": {"promptTokenCount": 5, "#,
        r#""candidatesTokenCount": 2, "totalTokenCount": 7}}"#
    );

    #[tokio::test]
    async fn test_streaming_text_response() {
        use futures::StreamExt;

        let lines: Vec<Result<String, anyhow::Error>> = GOOGLE_TEXT_STREAM
            .lines()
            .map(|l| Ok(l.to_string()))
            .collect();
        let stream = Box::pin(futures::stream::iter(lines));
        let mut message_stream = std::pin::pin!(response_to_streaming_message(stream));

        let mut text_parts = Vec::new();
        let mut message_ids: Vec<Option<String>> = Vec::new();
        let mut final_usage = None;

        while let Some(result) = message_stream.next().await {
            let (message, usage) = result.unwrap();
            if let Some(msg) = message {
                message_ids.push(msg.id.clone());
                if let Some(MessageContentBlock::Text(text)) = msg.content.first() {
                    text_parts.push(text.text.clone());
                }
            }
            if usage.is_some() {
                final_usage = usage;
            }
        }

        assert_eq!(text_parts, vec!["Hello", " world", "!"]);
        let usage = final_usage.unwrap();
        assert_eq!(usage.usage.input_tokens, Some(10));
        assert_eq!(usage.usage.output_tokens, Some(3));

        assert!(
            message_ids.iter().all(|id| id.is_some()),
            "All streaming messages should have an ID"
        );
        let first_id = message_ids.first().unwrap();
        assert!(
            message_ids.iter().all(|id| id == first_id),
            "All streaming messages should have the same ID"
        );
    }

    #[tokio::test]
    async fn test_streaming_response_metadata() {
        use futures::StreamExt;

        let lines = vec![Ok(
            r#"data: {"candidates":[{"content":{"role":"model","parts":[{"text":"done"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":2,"candidatesTokenCount":1,"totalTokenCount":3},"modelVersion":"gemini-test","responseId":"response-123"}"#
                .to_string(),
        )];
        let stream = Box::pin(futures::stream::iter(lines));
        let mut message_stream = std::pin::pin!(response_to_streaming_message(stream));
        let mut final_usage = None;

        while let Some(result) = message_stream.next().await {
            let (_, usage) = result.unwrap();
            if usage.is_some() {
                final_usage = usage;
            }
        }

        let usage = final_usage.unwrap();
        assert_eq!(usage.finish_reasons, Some(vec!["STOP".to_string()]));
        assert_eq!(usage.response_id.as_deref(), Some("response-123"));
    }

    #[tokio::test]
    async fn test_streaming_function_call() {
        use futures::StreamExt;

        let lines: Vec<Result<String, anyhow::Error>> = GOOGLE_FUNCTION_STREAM
            .lines()
            .map(|l| Ok(l.to_string()))
            .collect();
        let stream = Box::pin(futures::stream::iter(lines));
        let mut message_stream = std::pin::pin!(response_to_streaming_message(stream));

        let mut tool_calls = Vec::new();

        while let Some(result) = message_stream.next().await {
            let (message, _usage) = result.unwrap();
            if let Some(msg) = message {
                if let Some(MessageContentBlock::ToolRequest(req)) = msg.content.first() {
                    if let Ok(tool_call) = &req.tool_call {
                        tool_calls.push(tool_call.name.to_string());
                    }
                }
            }
        }

        assert_eq!(tool_calls, vec!["test_tool"]);
    }

    #[tokio::test]
    async fn test_streaming_with_thought_signature() {
        use futures::StreamExt;

        async fn collect_streaming_text(raw: &str) -> (String, usize) {
            let lines: Vec<Result<String, anyhow::Error>> =
                raw.lines().map(|l| Ok(l.to_string())).collect();
            let stream = Box::pin(futures::stream::iter(lines));
            let mut msg_stream = std::pin::pin!(response_to_streaming_message(stream));
            let mut text = String::new();
            let mut thinking = 0usize;
            while let Some(Ok((message, _))) = msg_stream.next().await {
                if let Some(msg) = message {
                    for c in &msg.content {
                        match c {
                            MessageContentBlock::Text(t) => text.push_str(&t.text),
                            MessageContentBlock::Thinking(_) => thinking += 1,
                            _ => {}
                        }
                    }
                }
            }
            (text, thinking)
        }

        let (text, thinking) = collect_streaming_text(concat!(
            r#"data: {"candidates": [{"content": {"role": "model", "#,
            r#""parts": [{"text": "Hello", "thoughtSignature": "sig1"}]}}], "#,
            r#""modelVersion": "gemini-3-flash-preview"}"#,
            "\n",
            r#"data: {"candidates": [{"content": {"role": "model", "#,
            r#""parts": [{"text": " world"}]}}], "modelVersion": "gemini-3-flash-preview"}"#
        ))
        .await;
        assert_eq!(thinking, 0);
        assert_eq!(text, "Hello world");

        let (text, thinking) = collect_streaming_text(concat!(
            r#"data: {"candidates": [{"content": {"role": "model", "#,
            r#""parts": [{"text": "SECURITY.md: Project"}]}}], "#,
            r#""modelVersion": "gemini-3-flash-preview"}"#,
            "\n",
            r#"data: {"candidates": [{"content": {"role": "model", "#,
            r#""parts": [{"text": " policies.\n\nRead it?", "thoughtSignature": "sig2"}]}}], "#,
            r#""modelVersion": "gemini-3-flash-preview"}"#
        ))
        .await;
        assert_eq!(thinking, 0);
        assert_eq!(text, "SECURITY.md: Project policies.\n\nRead it?");

        let (text, thinking) = collect_streaming_text(concat!(
            r#"data: {"candidates": [{"content": {"role": "model", "#,
            r#""parts": [{"text": "one "}]}}], "modelVersion": "gemini-3-flash-preview"}"#,
            "\n",
            r#"data: {"candidates": [{"content": {"role": "model", "#,
            r#""parts": [{"text": "two ", "thoughtSignature": "sig3"}]}}], "modelVersion": "gemini-3-flash-preview"}"#,
            "\n",
            r#"data: {"candidates": [{"content": {"role": "model", "#,
            r#""parts": [{"text": "three"}]}}], "modelVersion": "gemini-3-flash-preview"}"#
        ))
        .await;
        assert_eq!(thinking, 0);
        assert_eq!(text, "one two three");

        let (text, thinking) = collect_streaming_text(concat!(
            r#"data: {"candidates": [{"content": {"role": "model", "#,
            r#""parts": [{"text": "internal chain", "thought": true, "thoughtSignature": "sig4"}]}}]}"#,
            "\n",
            r#"data: {"candidates": [{"content": {"role": "model", "#,
            r#""parts": [{"text": "visible"}]}}]}"#
        ))
        .await;
        assert_eq!(thinking, 1);
        assert_eq!(text, "visible");
    }

    #[tokio::test]
    async fn test_streaming_error_response() {
        use futures::StreamExt;

        let error_stream = concat!(
            r#"data: {"error": {"code": 400, "#,
            r#""message": "Invalid request", "status": "INVALID_ARGUMENT"}}"#
        );
        let lines: Vec<Result<String, anyhow::Error>> =
            error_stream.lines().map(|l| Ok(l.to_string())).collect();
        let stream = Box::pin(futures::stream::iter(lines));
        let mut message_stream = std::pin::pin!(response_to_streaming_message(stream));

        let result = message_stream.next().await;
        assert!(result.is_some());
        let err = result.unwrap();
        assert!(err.is_err());
        let error_msg = err.unwrap_err().to_string();
        assert!(error_msg.contains("INVALID_ARGUMENT"));
        assert!(error_msg.contains("Invalid request"));
    }

    #[tokio::test]
    async fn test_streaming_with_sse_event_lines() {
        use futures::StreamExt;

        let sse_stream = r#"event: message
data: {"candidates": [{"content": {"role": "model", "parts": [{"text": "Hello"}]}}]}

event: message
data: {"candidates": [{"content": {"role": "model", "parts": [{"text": " world"}]}}]}

data: [DONE]"#;
        let lines: Vec<Result<String, anyhow::Error>> =
            sse_stream.lines().map(|l| Ok(l.to_string())).collect();
        let stream = Box::pin(futures::stream::iter(lines));
        let mut message_stream = std::pin::pin!(response_to_streaming_message(stream));

        let mut text_parts = Vec::new();

        while let Some(result) = message_stream.next().await {
            let (message, _usage) = result.unwrap();
            if let Some(msg) = message {
                if let Some(MessageContentBlock::Text(text)) = msg.content.first() {
                    text_parts.push(text.text.clone());
                }
            }
        }

        assert_eq!(text_parts, vec!["Hello", " world"]);
    }

    #[tokio::test]
    async fn test_streaming_handles_done_signal() {
        use futures::StreamExt;

        let stream_with_done = concat!(
            r#"data: {"candidates": [{"content": {"role": "model", "#,
            r#""parts": [{"text": "Complete"}]}}]}"#,
            "\n",
            "data: [DONE]\n",
            r#"data: {"candidates": [{"content": {"role": "model", "#,
            r#""parts": [{"text": "Should not appear"}]}}]}"#
        );
        let lines: Vec<Result<String, anyhow::Error>> = stream_with_done
            .lines()
            .map(|l| Ok(l.to_string()))
            .collect();
        let stream = Box::pin(futures::stream::iter(lines));
        let mut message_stream = std::pin::pin!(response_to_streaming_message(stream));

        let mut text_parts = Vec::new();

        while let Some(result) = message_stream.next().await {
            let (message, _usage) = result.unwrap();
            if let Some(msg) = message {
                if let Some(MessageContentBlock::Text(text)) = msg.content.first() {
                    text_parts.push(text.text.clone());
                }
            }
        }

        assert_eq!(text_parts, vec!["Complete"]);
    }

    #[test]
    fn test_format_tools_uses_parameters_json_schema() {
        let tool = Tool::new(
            "test_tool",
            "Test tool with $ref",
            object!({
                "type": "object",
                "$defs": {
                    "MyType": { "type": "string", "description": "A custom type" }
                },
                "properties": {
                    "field": { "$ref": "#/$defs/MyType" }
                }
            }),
        );

        let result = format_tools(&[tool]);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["name"], "test_tool");
        assert!(result[0].get("parametersJsonSchema").is_some());
        assert!(result[0].get("parameters").is_none());

        let schema = &result[0]["parametersJsonSchema"];
        assert_eq!(schema["properties"]["field"]["$ref"], "#/$defs/MyType");
        assert!(schema.get("$defs").is_some());
    }

    #[test]
    fn test_get_thinking_config_disabled_reasoning() {
        use crate::model::ModelConfig;

        let config = ModelConfig::new("gemini-2.5-flash").with_thinking_effort(ThinkingEffort::Off);
        let thinking_config = get_thinking_config(&config, None).unwrap();
        assert_eq!(thinking_config.thinking_budget, Some(0));
        assert!(!thinking_config.include_thoughts);

        let config = ModelConfig::new("gemini-2.5-pro").with_thinking_effort(ThinkingEffort::Off);
        assert!(get_thinking_config(&config, None).is_none());

        let config =
            ModelConfig::new("gemini-3.5-flash-lite").with_thinking_effort(ThinkingEffort::Off);
        let thinking_config = get_thinking_config(&config, None).unwrap();
        assert!(matches!(
            thinking_config.thinking_level,
            Some(ThinkingLevel::Minimal)
        ));
        assert!(!thinking_config.include_thoughts);
    }

    #[test]
    fn test_get_thinking_config() {
        use crate::model::ModelConfig;

        // Test 1: Gemini 3 model with low thinking effort
        let mut params = std::collections::HashMap::new();
        params.insert("thinking_effort".to_string(), serde_json::json!("low"));
        let mut config = ModelConfig::new("gemini-3-pro");
        config.request_params = Some(params);
        let result = get_thinking_config(&config, None);
        assert!(result.is_some());
        let thinking_config = result.unwrap();
        assert!(thinking_config.thinking_level.is_some());
        assert!(thinking_config.thinking_budget.is_none());
        assert!(thinking_config.include_thoughts);

        let config =
            ModelConfig::new("gemini-3.6-flash").with_thinking_effort(ThinkingEffort::Medium);
        let thinking_config = get_thinking_config(&config, None).unwrap();
        assert!(matches!(
            thinking_config.thinking_level,
            Some(ThinkingLevel::Medium)
        ));

        let config =
            ModelConfig::new("gemini-3-pro-preview").with_thinking_effort(ThinkingEffort::Medium);
        let thinking_config = get_thinking_config(&config, None).unwrap();
        assert!(matches!(
            thinking_config.thinking_level,
            Some(ThinkingLevel::Low)
        ));

        // Test 2: Gemini 3 model with high thinking effort
        let mut params = std::collections::HashMap::new();
        params.insert("thinking_effort".to_string(), serde_json::json!("high"));
        let mut config = ModelConfig::new("Gemini-3-Flash");
        config.request_params = Some(params);
        let result = get_thinking_config(&config, None);
        assert!(result.is_some());
        let thinking_config = result.unwrap();
        assert!(matches!(
            thinking_config.thinking_level,
            Some(ThinkingLevel::High)
        ));

        let config = ModelConfig::new("gemini-2.5-flash");
        let result = get_thinking_config(&config, None);
        assert!(result.is_some());
        let thinking_config = result.unwrap();
        assert!(thinking_config.include_thoughts);
        assert!(thinking_config.thinking_level.is_none());
        assert_eq!(
            thinking_config.thinking_budget,
            Some(DEFAULT_THINKING_BUDGET)
        );

        let mut params = HashMap::new();
        params.insert("thinking_budget".to_string(), json!(4096));
        let config = ModelConfig::new("gemini-2.5-flash").with_merged_request_params(params);
        let result = get_thinking_config(&config, None);
        assert!(result.is_some());
        let thinking_config = result.unwrap();
        assert_eq!(thinking_config.thinking_budget, Some(4096));

        let mut params = HashMap::new();
        params.insert("thinking_budget".to_string(), json!(-1));
        let config = ModelConfig::new("gemini-2.5-flash").with_merged_request_params(params);
        let result = get_thinking_config(&config, None);
        assert!(result.is_some());
        let thinking_config = result.unwrap();
        assert_eq!(
            thinking_config.thinking_budget,
            Some(DEFAULT_THINKING_BUDGET)
        );

        let config = ModelConfig::new("gemini-2.0-flash");
        let result = get_thinking_config(&config, None);
        assert!(result.is_none());

        let config = ModelConfig::new("gpt-4o");
        let result = get_thinking_config(&config, None);
        assert!(result.is_none());
    }

    #[test]
    fn test_gemini_3_request_omits_temperature() {
        let config = ModelConfig::new("gemini-3.6-flash").with_temperature(Some(0.2));
        let payload = create_request(&config, "system", &[], &[]).unwrap();

        assert!(payload["generationConfig"].get("temperature").is_none());
    }
}
