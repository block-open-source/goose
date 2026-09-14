use crate::base::ThinkingPreservationFormat;
use crate::conversation::message::{Message, MessageContentBlock, ProviderMetadata};
use crate::conversation::token_usage::{CostSource, ProviderUsage, Usage};
use crate::documents::{
    convert_document, document_media_type_is_supported, unsupported_document_text, DocumentFormat,
    ASSISTANT_ROLE_REASON, UNSUPPORTED_MEDIA_TYPE_REASON,
};
use crate::errors::ProviderError;
use crate::images::{convert_image, detect_image_path, load_image_file, ImageFormat};
use crate::json::{parse_tool_arguments, truncation_error_message};
use crate::mcp_utils::extract_text_from_resource;
use crate::model::{is_goose_internal_request_param, ModelConfig};
use crate::thinking::{
    split_think_blocks, ThinkFilter, ThinkingEffort, GEMINI_THOUGHT_SIGNATURE_KEY,
};
use anyhow::{anyhow, Error};
use async_stream::try_stream;
use chrono;
use futures::Stream;
use regex::Regex;
use rmcp::model::{object, CallToolRequestParams, ContentBlock, ErrorCode, ErrorData, Role, Tool};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::OnceLock;

type ToolCallData = HashMap<
    i32,
    (
        String,
        String,
        String,
        Option<serde_json::Map<String, Value>>,
    ),
>;

fn deserialize_null_default_string<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_default())
}

fn describe_json_value(value: &Value) -> &'static str {
    match value {
        Value::Array(_) => "an array",
        Value::String(_) => "a string",
        Value::Number(_) => "a number",
        Value::Bool(_) => "a boolean",
        Value::Null => "null",
        Value::Object(_) => "an object",
    }
}

fn output_token_limit_tool_error(function_name: &str, id: &str) -> ErrorData {
    ErrorData {
        code: ErrorCode::INVALID_PARAMS,
        message: Cow::from(format!(
            "Tool arguments for {function_name} (id {id}) were truncated because the model reached its output token limit"
        )),
        data: None,
    }
}

pub fn is_reserved_request_param_key(key: &str) -> bool {
    matches!(key, "messages" | "model" | "stream" | "stream_options")
}

#[derive(Debug, Clone, Copy, Default)]
pub struct OpenAiFormatOptions {
    pub preserve_thinking_context: bool,
    pub supports_vision: bool,
    pub thinking_preservation_format: Option<ThinkingPreservationFormat>,
}

fn merge_reasoning_text(prefix: &str, suffix: &str) -> String {
    if prefix.is_empty() {
        return suffix.to_string();
    }
    if suffix.is_empty() {
        return prefix.to_string();
    }
    if suffix.starts_with(prefix) {
        return suffix.to_string();
    }
    if prefix.ends_with(suffix) {
        return prefix.to_string();
    }

    format!("{prefix}{suffix}")
}

#[derive(Serialize, Deserialize, Debug, Default)]
struct DeltaToolCallFunction {
    name: Option<String>,
    #[serde(default, deserialize_with = "deserialize_null_default_string")]
    arguments: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct DeltaToolCall {
    id: Option<String>,
    function: DeltaToolCallFunction,
    index: Option<i32>,
    r#type: Option<String>,
    #[serde(flatten)]
    extra: Option<serde_json::Map<String, Value>>,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(untagged)]
enum DeltaContentBlock {
    String(String),
    Array(Vec<ContentBlockPart>),
}

#[derive(Serialize, Deserialize, Debug)]
struct ContentBlockPart {
    r#type: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(rename = "thoughtSignature")]
    thought_signature: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Default)]
struct Delta {
    #[serde(default)]
    content: Option<DeltaContentBlock>,
    role: Option<String>,
    tool_calls: Option<Vec<DeltaToolCall>>,
    reasoning_details: Option<Vec<Value>>,
    reasoning: Option<String>,
    reasoning_content: Option<String>,
}

impl Delta {
    /// Prefer `reasoning_content` (DeepSeek/OpenRouter) over `reasoning`
    /// (vLLM); some servers (gpt-oss via vLLM) emit both. Skip empty values.
    fn reasoning_text(&self) -> Option<&str> {
        self.reasoning_content
            .as_deref()
            .filter(|s| !s.is_empty())
            .or_else(|| self.reasoning.as_deref().filter(|s| !s.is_empty()))
    }
}

#[derive(Serialize, Deserialize, Debug)]
struct StreamingChoice {
    #[serde(default)]
    delta: Delta,
    index: Option<i32>,
    #[serde(default, deserialize_with = "empty_finish_reason_as_none")]
    finish_reason: Option<String>,
}

fn empty_finish_reason_as_none<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<String>::deserialize(deserializer)?;
    Ok(value.filter(|reason| !reason.is_empty()))
}

#[derive(Serialize, Deserialize, Debug)]
struct StreamingChunk {
    choices: Vec<StreamingChoice>,
    created: Option<i64>,
    id: Option<String>,
    usage: Option<Value>,
    model: Option<String>,
}

fn extract_content_and_signature(
    delta_content: Option<&DeltaContentBlock>,
) -> (Option<String>, Option<String>) {
    match delta_content {
        Some(DeltaContentBlock::String(s)) => (Some(s.clone()), None),
        Some(DeltaContentBlock::Array(parts)) => {
            let text_parts: Vec<_> = parts.iter().filter(|p| p.r#type == "text").collect();

            let text = text_parts
                .iter()
                .filter_map(|p| p.text.as_deref())
                .collect::<String>();

            let signature = text_parts
                .iter()
                .find_map(|p| p.thought_signature.as_ref())
                .cloned();

            let text = if text.is_empty() { None } else { Some(text) };

            (text, signature)
        }
        None => (None, None),
    }
}

pub fn format_messages(messages: &[Message], image_format: &ImageFormat) -> Vec<Value> {
    format_messages_with_options(
        messages,
        image_format,
        OpenAiFormatOptions {
            preserve_thinking_context: true,
            ..Default::default()
        },
    )
}

pub fn format_messages_with_options(
    messages: &[Message],
    image_format: &ImageFormat,
    options: OpenAiFormatOptions,
) -> Vec<Value> {
    let mut messages_spec = Vec::new();
    let mut pending_assistant_reasoning = String::new();
    // Reasoning to propagate across consecutive tool-call messages in the same turn.
    // DeepSeek/Kimi require reasoning_content on every assistant tool-call message.
    let mut tool_call_turn_reasoning = String::new();
    let mut saw_tool_response = false;

    for message in messages {
        if options.preserve_thinking_context && message.role != Role::Assistant {
            pending_assistant_reasoning.clear();
        }

        if options.preserve_thinking_context && message.role == Role::User {
            if message
                .content
                .iter()
                .any(|c| matches!(c, MessageContentBlock::ToolResponse(_)))
            {
                saw_tool_response = true;
            } else {
                tool_call_turn_reasoning.clear();
                saw_tool_response = false;
            }
        }

        // A new assistant message after tool results creates a new turn.
        // Prevents reasoning from the previous turn leaking into the new one.
        if options.preserve_thinking_context && message.role == Role::Assistant && saw_tool_response
        {
            tool_call_turn_reasoning.clear();
            saw_tool_response = false;
        }

        let mut converted = json!({
            "role": message.role
        });

        let mut output = Vec::new();
        let mut content_array = Vec::new();
        let mut has_non_text_content = false;
        let mut reasoning_text = String::new();

        for content in &message.content {
            match content {
                MessageContentBlock::Text(text) => {
                    if !text.text.is_empty() {
                        if message.role == Role::User {
                            if options.supports_vision {
                                if let Some(image_path) = detect_image_path(&text.text) {
                                    if let Ok(image) = load_image_file(image_path.as_ref()) {
                                        has_non_text_content = true;
                                        content_array
                                            .push(json!({"type": "text", "text": text.text}));
                                        content_array.push(convert_image(&image, image_format));
                                    } else {
                                        content_array
                                            .push(json!({"type": "text", "text": text.text}));
                                    }
                                } else {
                                    content_array.push(json!({"type": "text", "text": text.text}));
                                }
                            } else {
                                content_array.push(json!({"type": "text", "text": text.text}));
                            }
                        } else {
                            content_array.push(json!({"type": "text", "text": text.text}));
                        }
                    }
                }
                MessageContentBlock::Thinking(t) => {
                    reasoning_text.push_str(&t.thinking);
                }
                MessageContentBlock::RedactedThinking(_) => {
                    continue;
                }
                MessageContentBlock::SystemNotification(_) | MessageContentBlock::Error(_) => {
                    continue;
                }
                MessageContentBlock::ToolRequest(request) => match &request.tool_call {
                    Ok(tool_call) => {
                        let sanitized_name = sanitize_function_name(&tool_call.name);
                        let arguments_str = match &tool_call.arguments {
                            Some(args) => {
                                serde_json::to_string(args).unwrap_or_else(|_| "{}".to_string())
                            }
                            None => "{}".to_string(),
                        };

                        let tool_calls = converted
                            .as_object_mut()
                            .unwrap()
                            .entry("tool_calls")
                            .or_insert(json!([]));

                        let mut tool_call_json = json!({
                            "id": request.id,
                            "type": "function",
                            "function": {
                                "name": sanitized_name,
                                "arguments": arguments_str,
                            }
                        });

                        if let Some(metadata) = &request.metadata {
                            for (key, value) in metadata {
                                tool_call_json[key] = value.clone();
                            }
                        }

                        tool_calls.as_array_mut().unwrap().push(tool_call_json);
                    }
                    Err(_e) => {
                        // An unparseable tool call still needs a valid assistant
                        // `tool_calls` entry. Emitting the error as a bare `role:"tool"`
                        // message (the old behavior) leaves the paired tool response —
                        // which carries the parse error — as an orphan `role:"tool"` with
                        // no preceding assistant `tool_calls`, which strict
                        // OpenAI-compatible APIs reject. Emit a placeholder call with the
                        // same id so the history stays well-formed; the error rides on the
                        // following tool response.
                        let tool_calls = converted
                            .as_object_mut()
                            .unwrap()
                            .entry("tool_calls")
                            .or_insert(json!([]));
                        tool_calls.as_array_mut().unwrap().push(json!({
                            "id": request.id,
                            "type": "function",
                            "function": {
                                "name": "unparseable_tool_call",
                                "arguments": "{}",
                            }
                        }));
                    }
                },
                MessageContentBlock::ToolResponse(response) => {
                    match &response.tool_result {
                        Ok(result) => {
                            // Process all content, replacing images with placeholder text
                            let mut tool_content = Vec::new();
                            let mut image_messages = Vec::new();

                            for content in result.content.iter() {
                                match content {
                                    ContentBlock::Image(image) => {
                                        if options.supports_vision {
                                            // Add placeholder text in the tool response
                                            tool_content.push(ContentBlock::text("This tool result included an image that is uploaded in the next message."));

                                            // Create a separate image message
                                            image_messages.push(json!({
                                                "role": "user",
                                                "content": [convert_image(&image.clone(), image_format)]
                                            }));
                                        } else {
                                            // Add placeholder text in the tool response
                                            tool_content.push(ContentBlock::text("This tool result included an image that was omitted as the model does not support vision."));
                                        }
                                    }
                                    ContentBlock::Resource(resource) => {
                                        let text = extract_text_from_resource(&resource.resource);
                                        tool_content.push(ContentBlock::text(text));
                                    }
                                    _ => {
                                        tool_content.push(content.clone());
                                    }
                                }
                            }
                            let tool_response_content: Value = json!(tool_content
                                .iter()
                                .map(|content| match content {
                                    ContentBlock::Text(text) => text.text.clone(),
                                    _ => String::new(),
                                })
                                .collect::<Vec<String>>()
                                .join(" "));

                            // First add the tool response with all content
                            output.push(json!({
                                "role": "tool",
                                "content": tool_response_content,
                                "tool_call_id": response.id
                            }));
                            // Then add any image messages that need to follow
                            output.extend(image_messages);
                        }
                        Err(e) => {
                            // A tool result error is shown as output so the model can interpret the error message
                            output.push(json!({
                                "role": "tool",
                                "content": format!("The tool call returned the following error:\n{}", e),
                                "tool_call_id": response.id
                            }));
                        }
                    }
                }
                MessageContentBlock::ToolConfirmationRequest(_) => {}
                MessageContentBlock::ActionRequired(_) => {}
                MessageContentBlock::Image(image) => {
                    if message.role == Role::User {
                        if options.supports_vision {
                            has_non_text_content = true;
                            content_array.push(convert_image(image, image_format));
                        } else {
                            content_array.push(json!({
                                "type": "text",
                                "text": "[image omitted: model does not support vision]"
                            }));
                        }
                    } else {
                        content_array.push(json!({
                            "type": "text",
                            "text": "[Image content removed - not supported in assistant messages]"
                        }));
                    }
                }
                MessageContentBlock::Document(document) => {
                    if message.role != Role::User {
                        content_array.push(json!({
                            "type": "text",
                            "text": unsupported_document_text(document, ASSISTANT_ROLE_REASON)
                        }));
                    } else if document_media_type_is_supported(&document.mime_type) {
                        has_non_text_content = true;
                        content_array.push(convert_document(document, &DocumentFormat::OpenAi));
                    } else {
                        content_array.push(json!({
                            "type": "text",
                            "text": unsupported_document_text(document, UNSUPPORTED_MEDIA_TYPE_REASON)
                        }));
                    }
                }
            }
        }

        if !content_array.is_empty() {
            if has_non_text_content {
                converted["content"] = json!(content_array);
            } else {
                let texts: Vec<String> = content_array
                    .iter()
                    .filter_map(|v| v["text"].as_str().map(|s| s.to_string()))
                    .collect();
                converted["content"] = json!(texts.join("\n"));
            }
        }

        // Some strict OpenAI-compatible providers require "content" to be present
        // (even as null) when tool_calls are provided. See #6717.
        if message.role == Role::Assistant
            && converted.get("tool_calls").is_some()
            && converted.get("content").is_none()
        {
            converted["content"] = json!(null);
        }

        let has_message_payload =
            converted.get("content").is_some() || converted.get("tool_calls").is_some();

        if options.preserve_thinking_context && message.role == Role::Assistant {
            if !has_message_payload && output.is_empty() && !reasoning_text.is_empty() {
                pending_assistant_reasoning.push_str(&reasoning_text);
                continue;
            }

            if !pending_assistant_reasoning.is_empty() {
                reasoning_text =
                    merge_reasoning_text(&pending_assistant_reasoning, &reasoning_text);
                pending_assistant_reasoning.clear();
            }

            let has_tool_calls = converted
                .get("tool_calls")
                .and_then(|tc| tc.as_array())
                .is_some_and(|a| !a.is_empty());

            if has_tool_calls {
                if reasoning_text.is_empty() {
                    reasoning_text = tool_call_turn_reasoning.clone();
                } else {
                    tool_call_turn_reasoning = reasoning_text.clone();
                }
            } else {
                // Carry reasoning forward even through non-tool assistant messages
                // (e.g., a visible text chunk that's is sent before a tool-call chunk
                // in the same streaming turn). An empty reasoning_text is equivalent
                // to clear.
                tool_call_turn_reasoning = reasoning_text.clone();
            }
        }

        // Include reasoning_content only when non-empty. Kimi rejects empty
        // reasoning_content (""), so we must omit it entirely.
        if options.preserve_thinking_context && !reasoning_text.is_empty() {
            converted["reasoning_content"] = json!(reasoning_text);
        }

        if has_message_payload {
            output.insert(0, converted);
        }

        messages_spec.extend(output);
    }

    merge_split_tool_call_messages(&mut messages_spec);

    if let Some(format) = options.thinking_preservation_format {
        inline_reasoning_content(&mut messages_spec, format);
    }

    messages_spec
}

/// Rewrites `reasoning_content` into the message `content` for models that reject a
/// separate reasoning field on replay.
///
/// Must run after `merge_split_tool_call_messages`, which relies on `reasoning_content`
/// to identify messages split from the same assistant turn.
fn inline_reasoning_content(messages: &mut [Value], format: ThinkingPreservationFormat) {
    let wrap: fn(&str) -> String = match format {
        ThinkingPreservationFormat::ReasoningContent => return,
        ThinkingPreservationFormat::ContentPrepend => |text| format!("{text}\n\n"),
        ThinkingPreservationFormat::ContentXml => |text| format!("<think>\n{text}\n</think>\n\n"),
    };

    for message in messages {
        let Some(object) = message.as_object_mut() else {
            continue;
        };
        let Some(Value::String(reasoning)) = object.remove("reasoning_content") else {
            continue;
        };
        let prefix = wrap(&reasoning);

        match object.entry("content").or_insert(Value::Null) {
            Value::String(content) => content.insert_str(0, &prefix),
            Value::Array(blocks) => blocks.insert(0, json!({"type": "text", "text": prefix})),
            content => *content = json!(prefix.trim_end()),
        }
    }
}

/// The agent splits a single assistant response with N tool_calls into N
/// interleaved `asst(TC)/tool` pairs, cloning `reasoning_content` onto each.
/// This function merges them back into one assistant message with all tool_calls,
/// followed by the tool results — the standard OpenAI format.
///
/// Only merges when `reasoning_content` is present and matches, since that is
/// the only signal that messages were split from the same turn.
fn merge_split_tool_call_messages(messages: &mut Vec<Value>) {
    let mut i = 0;
    while i < messages.len() {
        let is_assistant_tool_call = messages[i].get("role") == Some(&json!("assistant"))
            && messages[i]
                .get("tool_calls")
                .and_then(|tc| tc.as_array())
                .is_some_and(|a| !a.is_empty());
        let base_reasoning = messages[i].get("reasoning_content");

        if !is_assistant_tool_call || base_reasoning.is_none() {
            i += 1;
            continue;
        }
        let base_reasoning = base_reasoning.unwrap().clone();

        let mut extra_tool_calls: Vec<Value> = Vec::new();
        let mut collected: Vec<Value> = Vec::new();
        let mut scan = i + 1;

        loop {
            if scan >= messages.len() || messages[scan].get("role") != Some(&json!("tool")) {
                break;
            }

            // Skip past tool result and any image-only user messages that
            // format_messages inserts after tool results containing images.
            let mut peek = scan + 1;
            while peek < messages.len() && is_image_only_user_message(&messages[peek]) {
                peek += 1;
            }

            if peek >= messages.len() {
                break;
            }
            let next = &messages[peek];
            let has_no_content = next.get("content").is_none_or(|c| {
                c.is_null()
                    || c.as_str().is_some_and(|s| s.is_empty())
                    || c.as_array().is_some_and(|a| a.is_empty())
            });
            let is_split = next.get("role") == Some(&json!("assistant"))
                && next
                    .get("tool_calls")
                    .and_then(|tc| tc.as_array())
                    .is_some_and(|a| !a.is_empty())
                && has_no_content
                && next.get("reasoning_content") == Some(&base_reasoning);

            if !is_split {
                break;
            }

            collected.extend(messages[scan..peek].iter().cloned());
            if let Some(tc) = messages[peek]
                .get("tool_calls")
                .and_then(|tc| tc.as_array())
            {
                extra_tool_calls.extend(tc.iter().cloned());
            }
            scan = peek + 1;
        }

        if extra_tool_calls.is_empty() {
            i += 1;
            continue;
        }

        if let Some(base_tc) = messages[i]
            .get_mut("tool_calls")
            .and_then(|tc| tc.as_array_mut())
        {
            base_tc.extend(extra_tool_calls);
        }

        let insert_at = i + 1;
        messages.drain(insert_at..scan);
        let num_collected = collected.len();
        for (j, msg) in collected.into_iter().enumerate() {
            messages.insert(insert_at + j, msg);
        }

        i = insert_at + num_collected;
    }
}

/// True if `msg` is a synthetic image-only user message (content is exclusively image_url items).
fn is_image_only_user_message(msg: &Value) -> bool {
    msg.get("role") == Some(&json!("user"))
        && msg
            .get("content")
            .and_then(|c| c.as_array())
            .is_some_and(|arr| {
                !arr.is_empty()
                    && arr
                        .iter()
                        .all(|item| item.get("type") == Some(&json!("image_url")))
            })
}

pub fn format_tools(tools: &[Tool]) -> anyhow::Result<Vec<Value>> {
    let mut tool_names = std::collections::HashSet::new();
    let mut result = Vec::new();

    for tool in tools {
        if !tool_names.insert(&tool.name) {
            return Err(anyhow!("Duplicate tool name: {}", tool.name));
        }

        result.push(json!({
            "type": "function",
            "function": {
                "name": tool.name,
                "description": tool.description,
                "parameters": tool.input_schema,
            }
        }));
    }

    Ok(result)
}

pub fn record_response_metadata(usage: &mut ProviderUsage, response: &Value) {
    usage.response_id = response
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string);

    let finish_reasons = response
        .get("choices")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|choice| choice.get("finish_reason").and_then(Value::as_str))
        .map(str::to_string)
        .collect::<Vec<_>>();
    if !finish_reasons.is_empty() {
        usage.finish_reasons = Some(finish_reasons);
    }
}

/// Convert OpenAI's API response to internal Message format
pub fn response_to_message(response: &Value) -> anyhow::Result<Message> {
    let output_token_limit_reached = response
        .pointer("/choices/0/finish_reason")
        .and_then(Value::as_str)
        == Some("length");

    let Some(original) = response
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|m| m.get("message"))
    else {
        if let Some(error) = response.get("error") {
            let error_message = error
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("Unknown error");
            return Err(anyhow::anyhow!("API error: {}", error_message));
        }
        return Err(anyhow::anyhow!(
            "No message in API response. This may indicate a quota limit or other restriction."
        ));
    };

    let mut content = Vec::new();

    // Capture reasoning content if present (DeepSeek uses "reasoning_content", vLLM uses "reasoning")
    let reasoning_value = original
        .get("reasoning_content")
        .or_else(|| original.get("reasoning"));
    let mut has_structured_thinking = false;
    if let Some(reasoning_content) = reasoning_value {
        if let Some(reasoning_str) = reasoning_content.as_str() {
            if !reasoning_str.is_empty() {
                has_structured_thinking = true;
                content.push(MessageContentBlock::thinking(reasoning_str, ""));
            }
        }
    }

    if let Some(text) = original.get("content") {
        if let Some(text_str) = text.as_str() {
            let (cleaned, inline_thinking) = split_think_blocks(text_str);

            if !has_structured_thinking && !inline_thinking.is_empty() {
                content.push(MessageContentBlock::thinking(inline_thinking, ""));
            }

            if !cleaned.is_empty() {
                content.push(MessageContentBlock::text(cleaned));
            }
        }
    }

    if let Some(tool_calls) = original.get("tool_calls") {
        if let Some(tool_calls_array) = tool_calls.as_array() {
            for tool_call in tool_calls_array {
                let id = tool_call["id"].as_str().unwrap_or_default().to_string();
                let function_name = tool_call["function"]["name"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();

                // Get the raw arguments string from the LLM.
                let arguments_str = tool_call["function"]["arguments"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();

                // If arguments_str is empty, default to an empty JSON object string.
                let arguments_str = if arguments_str.is_empty() {
                    "{}".to_string()
                } else {
                    arguments_str
                };

                let standard_fields = ["id", "function", "type", "index"];
                let metadata: Option<serde_json::Map<String, Value>> = tool_call
                    .as_object()
                    .map(|obj| {
                        obj.iter()
                            .filter(|(k, _)| !standard_fields.contains(&k.as_str()))
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect()
                    })
                    .filter(|m: &serde_json::Map<String, Value>| !m.is_empty());

                if output_token_limit_reached {
                    let error = output_token_limit_tool_error(&function_name, &id);
                    content.push(MessageContentBlock::tool_request_with_metadata(
                        id,
                        Err(error),
                        metadata.as_ref(),
                    ));
                    continue;
                }

                if function_name.is_empty() {
                    let error = ErrorData {
                        code: ErrorCode::INVALID_REQUEST,
                        message: Cow::from(
                            "The provided function name was empty; a tool call must name a tool"
                                .to_string(),
                        ),
                        data: None,
                    };
                    content.push(MessageContentBlock::tool_request_with_metadata(
                        id,
                        Err(error),
                        metadata.as_ref(),
                    ));
                    continue;
                }
                match parse_tool_arguments(&arguments_str) {
                    Some(params) if params.is_object() => {
                        content.push(MessageContentBlock::tool_request_with_metadata(
                            id,
                            Ok(CallToolRequestParams::new(function_name)
                                .with_arguments(object(params))),
                            metadata.as_ref(),
                        ));
                    }
                    Some(other) => {
                        let error = ErrorData {
                            code: ErrorCode::INVALID_PARAMS,
                            message: Cow::from(format!(
                                "Tool arguments for {} (id {}) must be a JSON object, got {}. Raw arguments: '{}'",
                                function_name,
                                id,
                                describe_json_value(&other),
                                arguments_str
                            )),
                            data: None,
                        };
                        content.push(MessageContentBlock::tool_request_with_metadata(
                            id,
                            Err(error),
                            metadata.as_ref(),
                        ));
                    }
                    None => {
                        let message_text =
                            truncation_error_message(&arguments_str).unwrap_or_else(|| {
                                format!("Could not interpret tool use parameters for id {id}")
                            });
                        let error = ErrorData {
                            code: ErrorCode::INVALID_PARAMS,
                            message: Cow::from(message_text),
                            data: None,
                        };
                        content.push(MessageContentBlock::tool_request_with_metadata(
                            id,
                            Err(error),
                            metadata.as_ref(),
                        ));
                    }
                }
            }
        }
    }

    let mut message = Message::new(Role::Assistant, chrono::Utc::now().timestamp(), content);
    message.metadata.output_token_limit_reached = output_token_limit_reached;
    Ok(message)
}

pub fn get_usage(usage: &Value) -> Usage {
    let usage = usage
        .get("usage")
        .filter(|nested| nested.is_object())
        .unwrap_or(usage);

    // Try standard OpenAI fields first, then fall back to Ollama-native fields
    // (prompt_eval_count / eval_count) for compatibility with older Ollama builds
    // that don't translate to OpenAI field names.
    // Parse the value before falling back so that present-but-null keys
    // (e.g. "completion_tokens": null) don't block the fallback.
    let input_tokens = usage
        .get("prompt_tokens")
        .and_then(|v| v.as_i64())
        .or_else(|| usage.get("prompt_eval_count").and_then(|v| v.as_i64()))
        .map(|v| v as i32);

    let output_tokens = usage
        .get("completion_tokens")
        .and_then(|v| v.as_i64())
        .or_else(|| usage.get("eval_count").and_then(|v| v.as_i64()))
        .map(|v| v as i32);

    let cache_read_input_tokens = usage
        .get("cache_read_input_tokens")
        .and_then(|v| v.as_i64())
        .or_else(|| {
            usage
                .get("prompt_tokens_details")
                .and_then(|d| d.get("cached_tokens"))
                .and_then(|v| v.as_i64())
        })
        .map(|v| v as i32);

    let cache_write_input_tokens = usage
        .get("cache_creation_input_tokens")
        .and_then(|v| v.as_i64())
        .or_else(|| {
            usage
                .get("prompt_tokens_details")
                .and_then(|d| d.get("cache_write_tokens"))
                .and_then(|v| v.as_i64())
        })
        .map(|v| v as i32);

    let total_tokens = usage
        .get("total_tokens")
        .and_then(|v| v.as_i64())
        .map(|v| v as i32)
        .or_else(|| match (input_tokens, output_tokens) {
            (Some(input), Some(output)) => Some(input.saturating_add(output)),
            _ => None,
        });

    Usage::new(input_tokens, output_tokens, total_tokens)
        .with_cache_tokens(cache_read_input_tokens, cache_write_input_tokens)
}

pub fn get_cost(usage: &Value) -> Option<f64> {
    usage
        .get("cost")
        .and_then(|v| v.as_f64())
        .filter(|c| c.is_finite() && *c >= 0.0)
}

fn extract_usage_with_output_tokens(
    chunk: &StreamingChunk,
    fallback_model: Option<&str>,
) -> Option<ProviderUsage> {
    chunk
        .usage
        .as_ref()
        .and_then(|u| {
            chunk.model.as_deref().or(fallback_model).map(|model| {
                let usage = ProviderUsage::new(model.to_string(), get_usage(u));
                match get_cost(u) {
                    Some(cost) => usage.with_cost(cost, CostSource::ProviderReported),
                    None => usage,
                }
            })
        })
        .filter(|u| u.usage.output_tokens.is_some())
}

/// Validates and fixes tool schemas to ensure they have proper parameter structure.
/// If parameters exist, ensures they have properties and required fields, or removes parameters entirely.
pub fn validate_tool_schemas(tools: &mut [Value]) {
    for tool in tools.iter_mut() {
        if let Some(function) = tool.get_mut("function") {
            if let Some(parameters) = function.get_mut("parameters") {
                if parameters.is_object() {
                    ensure_valid_json_schema(parameters);
                }
            }
        }
    }
}

/// Ensures that the given JSON value follows the expected JSON Schema structure.
fn ensure_valid_json_schema(schema: &mut Value) {
    if let Some(params_obj) = schema.as_object_mut() {
        if !params_obj.contains_key("type") {
            params_obj.insert("type".to_string(), json!("object"));
        }
    }
    sanitize_schema_node(schema);
}

fn sanitize_schema_node(node: &mut Value) {
    if let Some(obj) = node.as_object_mut() {
        // Moonshot's walle validator rejects `oneOf` behind a `$ref` as
        // "infinite recursion" because its termination check only traverses
        // `anyOf`. The two are interchangeable for tool-argument schemas, so
        // emit the more widely supported form.
        if !obj.contains_key("anyOf") {
            if let Some(one_of) = obj.remove("oneOf") {
                obj.insert("anyOf".to_string(), one_of);
            }
        }
    }

    normalize_nullable(node);

    let Some(obj) = node.as_object_mut() else {
        return;
    };

    if obj.get("type").and_then(|t| t.as_str()) == Some("object") {
        obj.entry("properties").or_insert_with(|| json!({}));
        obj.entry("required").or_insert_with(|| json!([]));
    }

    for key in ["properties", "$defs", "definitions"] {
        if let Some(children) = obj.get_mut(key).and_then(Value::as_object_mut) {
            for child in children.values_mut() {
                sanitize_schema_node(child);
            }
        }
    }
    for key in ["anyOf", "allOf", "prefixItems"] {
        if let Some(children) = obj.get_mut(key).and_then(Value::as_array_mut) {
            for child in children.iter_mut() {
                sanitize_schema_node(child);
            }
        }
    }
    for key in ["items", "additionalProperties"] {
        if let Some(child) = obj.get_mut(key) {
            if child.is_object() {
                sanitize_schema_node(child);
            }
        }
    }
}

/// Normalizes nullable type representations that some providers (e.g. Vertex Gemini via Bifrost)
/// don't support:
/// - `"type": ["integer", "null"]` → `"type": "integer"` (drops the null variant)
/// - `"anyOf": [T, {"type": "null"}]` → T (unwraps to the non-null schema)
///
/// Optional-ness is already conveyed by the field being absent from `required`.
fn normalize_nullable(schema: &mut Value) {
    let Some(obj) = schema.as_object_mut() else {
        return;
    };

    // Handle type: ["T", "null"] array form (schemars 1.x style for nullable primitives)
    if let Some(type_val) = obj.get("type").cloned() {
        if let Some(types) = type_val.as_array() {
            let non_null: Vec<&Value> = types
                .iter()
                .filter(|t| t.as_str() != Some("null"))
                .collect();
            if non_null.len() == 1 {
                let scalar = non_null[0].clone();
                obj.insert("type".to_string(), scalar);
                return;
            }
        }
    }

    // Handle anyOf: [T, {type: "null"}] form — merge the non-null variant's fields
    // into the current object (preserving sibling keys like "description" or "default")
    // rather than replacing the whole schema.
    if let Some(any_of) = obj.remove("anyOf") {
        if let Some(variants) = any_of.as_array() {
            if variants.len() == 2 {
                let is_null = |v: &Value| v.get("type").and_then(|t| t.as_str()) == Some("null");
                let non_null = if is_null(&variants[0]) {
                    Some(&variants[1])
                } else if is_null(&variants[1]) {
                    Some(&variants[0])
                } else {
                    None
                };
                if let Some(replacement) = non_null {
                    if let Some(replacement_obj) = replacement.as_object() {
                        for (k, v) in replacement_obj {
                            obj.entry(k.clone()).or_insert(v.clone());
                        }
                        return;
                    }
                }
            }
        }
        // Put it back if we couldn't simplify
        obj.insert("anyOf".to_string(), any_of);
    }
}

fn strip_data_prefix(line: &str) -> Option<&str> {
    // SSE spec allows both "data: value" and "data:value" (space after colon is optional)
    line.strip_prefix("data: ")
        .or_else(|| line.strip_prefix("data:"))
        .map(|s| s.trim())
}

/// Longest error text pulled out of a stream frame, so a pathological payload cannot be
/// pasted wholesale into a user-facing message.
const MAX_STREAM_ERROR_LEN: usize = 500;

/// Best-effort human-readable text for an error payload that may not be a plain string.
///
/// FastAPI reports `HTTPException` as `{"detail": "..."}` but `RequestValidationError` as
/// `{"detail": [{"loc": [...], "msg": "field required", ...}]}`, so a string-only read would
/// drop the commoner validation shape entirely.
fn stream_error_text(value: &Value) -> Option<String> {
    fn one(value: &Value) -> Option<String> {
        match value {
            Value::String(s) => Some(s.clone()),
            Value::Object(_) => value
                .get("msg")
                .or_else(|| value.get("message"))
                .and_then(|m| m.as_str().map(String::from))
                .or_else(|| Some(value.to_string())),
            Value::Null => None,
            other => Some(other.to_string()),
        }
    }

    let text = match value {
        Value::Array(items) => {
            let parts: Vec<String> = items.iter().filter_map(one).collect();
            if parts.is_empty() {
                return None;
            }
            parts.join("; ")
        }
        other => one(other)?,
    };
    if text.is_empty() {
        return None;
    }
    if text.chars().count() > MAX_STREAM_ERROR_LEN {
        let truncated: String = text.chars().take(MAX_STREAM_ERROR_LEN).collect();
        return Some(format!("{truncated}…"));
    }
    Some(text)
}

/// Decide whether a choice-less SSE frame reports an in-stream failure.
///
/// Returns `Some(err)` when it does, `None` when it is gateway metadata that can be skipped.
///
/// Requires an actual error *signal* — a `status`/`statusCode`/`code` of 400 or above, a
/// `type` of `"error"`, or a `detail` field, which has no benign meaning in this position.
/// Mere prose is not enough: gateways also emit informational frames, and treating
/// `{"message": "processing"}` as a failure would kill a healthy stream, which is the very
/// bug this skip exists to avoid. The converse matters just as much — a gateway that
/// rate-limits with a bare `{"statusCode": 429, "message": …}` on an HTTP 200 must not be
/// silently skipped, or a failed turn is reported as an empty successful one.
fn classify_choiceless_frame(value: &Value) -> Option<ProviderError> {
    let status = ["status", "statusCode", "code"].iter().find_map(|key| {
        let raw = value.get(*key)?;
        raw.as_i64()
            .or_else(|| raw.as_str().and_then(|s| s.parse::<i64>().ok()))
    });

    let has_error_signal = status.is_some_and(|s| s >= 400)
        || value.get("type").and_then(|t| t.as_str()) == Some("error")
        || value.get("detail").is_some_and(|d| !d.is_null());
    if !has_error_signal {
        return None;
    }

    let details = value
        .get("message")
        .and_then(stream_error_text)
        .or_else(|| value.get("detail").and_then(stream_error_text))
        .or_else(|| value.get("error").and_then(stream_error_text))
        // A status with no recoverable text must still be loud rather than vanish.
        .unwrap_or_else(|| match status {
            Some(s) => format!("Gateway returned status {s} mid-stream"),
            None => "Unknown server error".to_string(),
        });
    Some(ProviderError::ServerError(details))
}

/// Parse one SSE `data:` payload.
///
/// Returns `Ok(None)` for a metadata-only frame — a JSON object with no `choices` key at
/// all. Gateways interleave these with the real chunks: Portkey/Azure APIM and friends
/// emit trace/guardrail objects such as `{"hook_results": {...}}` before the first token.
/// They carry nothing this parser consumes, so they are skipped rather than failed on;
/// treating them as decode errors kills the whole turn on an otherwise healthy stream.
///
/// A frame with `"choices": []` is NOT metadata — that is the standard usage-only chunk,
/// so it still deserializes and flows through the empty-choices paths below.
///
/// A choice-less frame that reports an in-stream failure is NOT metadata either — see
/// `classify_choiceless_frame`.
fn parse_streaming_chunk(line: &str) -> Result<Option<StreamingChunk>, ProviderError> {
    let value: Value = serde_json::from_str(line).map_err(|e| {
        ProviderError::stream_decode_error(format!(
            "Failed to parse streaming chunk: {e}: {line:?}"
        ))
    })?;

    if let Some(error) = value.get("error") {
        let message = error
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("Unknown server error");
        return Err(ProviderError::ServerError(message.to_string()));
    }

    if value.get("object").and_then(|o| o.as_str()) == Some("error") {
        let message = value
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("Unknown server error");
        return Err(ProviderError::ServerError(message.to_string()));
    }

    if value
        .as_object()
        .is_some_and(|o| !o.contains_key("choices"))
    {
        if let Some(err) = classify_choiceless_frame(&value) {
            return Err(err);
        }
        return Ok(None);
    }

    serde_json::from_value(value).map(Some).map_err(|e| {
        ProviderError::stream_decode_error(format!(
            "Failed to parse streaming chunk: {e}: {line:?}"
        ))
    })
}

fn output_token_limit_marker(id: Option<String>) -> Message {
    let mut message = Message::assistant();
    if let Some(id) = id {
        message = message.with_id(id);
    }
    message.metadata.output_token_limit_reached = true;
    message
}

pub fn response_to_streaming_message<S>(
    mut stream: S,
) -> impl Stream<Item = anyhow::Result<(Option<Message>, Option<ProviderUsage>)>> + 'static
where
    S: Stream<Item = anyhow::Result<String>> + Unpin + Send + 'static,
{
    try_stream! {
        use futures::StreamExt;

        let mut accumulated_reasoning: Vec<Value> = Vec::new();
        let mut accumulated_reasoning_content = String::new();
        let mut think_filter = ThinkFilter::new();
        let mut saw_structured_reasoning = false;
        let mut yielded_reasoning_content_len = 0usize;
        let mut last_signature: Option<String> = None;
        // Buffer inline <think>...</think> content until we know whether structured
        // reasoning will arrive. Emitting it immediately and then receiving
        // reasoning_content in a later chunk would produce duplicated reasoning.
        let mut pending_inline_thinking = String::new();
        let mut last_seen_model: Option<String> = None;
        let mut last_response_id: Option<String> = None;
        let mut last_finish_reason: Option<String> = None;
        let mut output_token_limit_reached = false;
        let mut output_token_limit_metadata_emitted = false;
        let mut usage_emitted = false;

        'outer: while let Some(response) = stream.next().await {
            let response_str = response?;
            let line = strip_data_prefix(&response_str);

            if line.is_some_and(|l| l == "[DONE]") {
                break 'outer;
            }

            if line.is_none() || line.is_some_and(|l| l.is_empty()) {
                continue
            }

            let Some(chunk) = parse_streaming_chunk(
                line.ok_or_else(|| anyhow!("unexpected stream format"))?
            )? else {
                continue  // metadata-only frame
            };
            if let Some(model) = &chunk.model {
                last_seen_model = Some(model.clone());
            }
            if let Some(id) = &chunk.id {
                last_response_id = Some(id.clone());
            }

            if !chunk.choices.is_empty() {
                if let Some(details) = &chunk.choices[0].delta.reasoning_details {
                    accumulated_reasoning.extend(details.iter().cloned());
                }
                if let Some(rc) = chunk.choices[0].delta.reasoning_text() {
                    accumulated_reasoning_content.push_str(rc);
                    if !rc.is_empty() {
                        saw_structured_reasoning = true;
                        pending_inline_thinking.clear();
                    }
                }
            }

            if let Some(reason) = chunk.choices.first().and_then(|c| c.finish_reason.clone()) {
                last_finish_reason = Some(reason);
            }
            let mut usage = extract_usage_with_output_tokens(&chunk, last_seen_model.as_deref());
            if let Some(u) = usage.as_mut() {
                if let Some(reason) = &last_finish_reason {
                    u.finish_reasons = Some(vec![reason.clone()]);
                }
                if let Some(id) = &last_response_id {
                    u.response_id = Some(id.clone());
                }
            }
            output_token_limit_reached |= last_finish_reason.as_deref() == Some("length");

            if chunk.choices.is_empty() {
                usage_emitted |= usage.is_some();
                yield (None, usage)
            } else if chunk.choices[0].delta.tool_calls.as_ref().is_some_and(|tc| !tc.is_empty()) {
                let mut tool_call_data: ToolCallData = HashMap::new();

                if let Some(tool_calls) = &chunk.choices[0].delta.tool_calls {
                    for (position, tool_call) in tool_calls.iter().enumerate() {
                        if let (Some(id), Some(name)) = (&tool_call.id, &tool_call.function.name) {
                            let index = tool_call.index.unwrap_or(position as i32);
                            tool_call_data.insert(index, (id.clone(), name.clone(), tool_call.function.arguments.clone(), tool_call.extra.clone()));
                        }
                    }
                }

                let is_complete = matches!(
                    chunk.choices[0].finish_reason.as_deref(),
                    Some("tool_calls" | "length")
                );

                if !is_complete {
                    let mut done = false;
                    while !done {
                        if let Some(response_chunk) = stream.next().await {
                            let response_str = response_chunk?;
                            if let Some(line) = strip_data_prefix(&response_str) {
                                if line == "[DONE]" {
                                    break 'outer;
                                }

                                // A metadata frame here must NOT fall through to the
                                // empty-choices branch below, which ends accumulation and
                                // would truncate this tool call's arguments.
                                let Some(tool_chunk) = parse_streaming_chunk(line)? else {
                                    continue
                                };
                                if let Some(model) = &tool_chunk.model {
                                    last_seen_model = Some(model.clone());
                                }
                                if let Some(id) = &tool_chunk.id {
                                    last_response_id = Some(id.clone());
                                }
                                if let Some(reason) = tool_chunk.choices.first().and_then(|c| c.finish_reason.clone()) {
                                    last_finish_reason = Some(reason);
                                }

                                if let Some(mut chunk_usage) = extract_usage_with_output_tokens(&tool_chunk, last_seen_model.as_deref()) {
                                    if let Some(reason) = &last_finish_reason {
                                        chunk_usage.finish_reasons = Some(vec![reason.clone()]);
                                    }
                                    if let Some(id) = &last_response_id {
                                        chunk_usage.response_id = Some(id.clone());
                                    }
                                    usage = Some(chunk_usage);
                                }

                                if !tool_chunk.choices.is_empty() {
                                    output_token_limit_reached |=
                                        tool_chunk.choices[0].finish_reason.as_deref()
                                            == Some("length");

                                    if let Some(details) = &tool_chunk.choices[0].delta.reasoning_details {
                                        accumulated_reasoning.extend(details.iter().cloned());
                                    }
                                    if let Some(rc) = tool_chunk.choices[0].delta.reasoning_text() {
                                        accumulated_reasoning_content.push_str(rc);
                                        if !rc.is_empty() {
                                            saw_structured_reasoning = true;
                                            pending_inline_thinking.clear();
                                        }
                                    }
                                    if let Some(delta_tool_calls) = &tool_chunk.choices[0].delta.tool_calls {
                                        for delta_call in delta_tool_calls {
                                            if let Some(index) = delta_call.index {
                                                if let Some((_, ref mut stored_name, ref mut args, ref mut extra)) = tool_call_data.get_mut(&index) {
                                                    if let Some(new_name) = &delta_call.function.name {
                                                        if !new_name.is_empty() {
                                                            *stored_name = new_name.clone();
                                                        }
                                                    }
                                                    args.push_str(&delta_call.function.arguments);
                                                    if extra.is_none() && delta_call.extra.is_some() {
                                                        *extra = delta_call.extra.clone();
                                                    } else if let (Some(existing), Some(new_extra)) = (extra.as_mut(), &delta_call.extra) {
                                                        for (key, value) in new_extra {
                                                            existing.entry(key.clone()).or_insert(value.clone());
                                                        }
                                                    }
                                                } else if let (Some(id), Some(name)) = (&delta_call.id, &delta_call.function.name) {
                                                    tool_call_data.insert(index, (id.clone(), name.clone(), delta_call.function.arguments.clone(), delta_call.extra.clone()));
                                                }
                                            }
                                        }
                                    }
                                    if tool_chunk.choices[0].finish_reason.is_some() {
                                        done = true;
                                    }
                                } else {
                                    done = true;
                                }
                            }
                        } else {
                            break;
                        }
                    }
                }

                let _metadata: Option<ProviderMetadata> = if !accumulated_reasoning.is_empty() {
                    let mut map = ProviderMetadata::new();
                    map.insert("reasoning_details".to_string(), json!(accumulated_reasoning));
                    Some(map)
                } else {
                    None
                };

                let filtered = think_filter.push("");
                let mut flush_thinking = String::new();
                if !saw_structured_reasoning {
                    flush_thinking.push_str(&pending_inline_thinking);
                    flush_thinking.push_str(&filtered.thinking);
                }
                pending_inline_thinking.clear();
                if !filtered.content.is_empty() || !flush_thinking.is_empty() {
                    let mut filtered_contents = Vec::new();
                    if !filtered.content.is_empty() {
                        filtered_contents.push(MessageContentBlock::text(filtered.content));
                    }
                    if !flush_thinking.is_empty() {
                        filtered_contents.push(MessageContentBlock::thinking(flush_thinking, ""));
                    }

                    if !filtered_contents.is_empty() {
                        let mut msg = Message::new(
                            Role::Assistant,
                            chrono::Utc::now().timestamp(),
                            filtered_contents,
                        );

                        if let Some(id) = chunk.id.clone() {
                            msg = msg.with_id(id);
                        }

                        yield (Some(msg), None);
                    }
                }

                let mut contents = Vec::new();
                if yielded_reasoning_content_len < accumulated_reasoning_content.len() {
                    if let Some(unyielded_reasoning) =
                        accumulated_reasoning_content.get(yielded_reasoning_content_len..)
                    {
                        if !unyielded_reasoning.is_empty() {
                            contents.push(MessageContentBlock::thinking(unyielded_reasoning, ""));
                        }
                    }
                }
                accumulated_reasoning_content.clear();
                yielded_reasoning_content_len = 0;
                let mut sorted_indices: Vec<_> = tool_call_data.keys().cloned().collect();
                sorted_indices.sort();

                for index in sorted_indices {
                    if let Some((id, function_name, arguments, extra_fields)) = tool_call_data.get(&index) {
                        let metadata = if let Some(sig) = &last_signature {
                            let mut combined = extra_fields.clone().unwrap_or_default();
                            combined.insert(
                                GEMINI_THOUGHT_SIGNATURE_KEY.to_string(),
                                json!(sig)
                            );
                            Some(combined)
                        } else {
                            extra_fields.as_ref().filter(|m| !m.is_empty()).cloned()
                        };

                        let content = if output_token_limit_reached {
                            MessageContentBlock::tool_request_with_provider_index(
                                id.clone(),
                                Err(output_token_limit_tool_error(function_name, id)),
                                metadata.as_ref(),
                                index,
                            )
                        } else if arguments.is_empty() {
                            MessageContentBlock::tool_request_with_provider_index(
                                id.clone(),
                                Ok(CallToolRequestParams::new(function_name.clone()).with_arguments(object(json!({})))),
                                metadata.as_ref(),
                                index,
                            )
                        } else {
                            match parse_tool_arguments(arguments) {
                                Some(params) if params.is_object() => MessageContentBlock::tool_request_with_provider_index(
                                    id.clone(),
                                    Ok(CallToolRequestParams::new(function_name.clone()).with_arguments(object(params))),
                                    metadata.as_ref(),
                                    index,
                                ),
                                // Valid JSON but NOT an object (a bare array/string/number).
                                // Surface a tool error so the model retries instead of
                                // crashing the run (rmcp's `object()` debug-asserts on
                                // non-objects). Mirrors the non-streaming decoder.
                                Some(other) => {
                                    let error = ErrorData {
                                        code: ErrorCode::INVALID_PARAMS,
                                        message: Cow::from(format!(
                                            "Tool arguments for {} (id {}) must be a JSON object, got {}. Raw arguments: '{}'",
                                            function_name, id, describe_json_value(&other), arguments
                                        )),
                                        data: None,
                                    };
                                    MessageContentBlock::tool_request_with_provider_index(id.clone(), Err(error), metadata.as_ref(), index)
                                }
                                None => {
                                    let message_text = truncation_error_message(arguments)
                                        .unwrap_or_else(|| {
                                            format!("Could not interpret tool use parameters for id {id}")
                                        });
                                    let error = ErrorData {
                                        code: ErrorCode::INVALID_PARAMS,
                                        message: Cow::from(message_text),
                                        data: None,
                                    };
                                    MessageContentBlock::tool_request_with_provider_index(id.clone(), Err(error), metadata.as_ref(), index)
                                }
                            }
                        };

                        contents.push(content);
                    }
                }

                let mut msg = Message::new(
                    Role::Assistant,
                    chrono::Utc::now().timestamp(),
                    contents,
                );

                // Add ID if present
                if let Some(id) = chunk.id {
                    msg = msg.with_id(id);
                }
                msg.metadata.output_token_limit_reached = output_token_limit_reached;
                output_token_limit_metadata_emitted |= output_token_limit_reached;

                usage_emitted |= usage.is_some();
                yield (
                    Some(msg),
                    usage,
                )
            } else if chunk.choices[0].delta.content.is_some() || chunk.choices[0].delta.reasoning_text().is_some() {
                let mut content = Vec::new();

                if let Some(reasoning) = chunk.choices[0].delta.reasoning_text() {
                    let signature = last_signature.as_deref().unwrap_or("");
                    content.push(MessageContentBlock::thinking(reasoning, signature));
                    yielded_reasoning_content_len = accumulated_reasoning_content.len();
                }

                let (text_content, thought_signature) = extract_content_and_signature(chunk.choices[0].delta.content.as_ref());

                if let Some(sig) = thought_signature {
                    last_signature = Some(sig);
                }

                if let Some(text) = text_content {
                    let filtered = think_filter.push(&text);

                    if !saw_structured_reasoning && !filtered.thinking.is_empty() {
                        pending_inline_thinking.push_str(&filtered.thinking);
                    }

                    if !filtered.content.is_empty() {
                        content.push(MessageContentBlock::text(filtered.content));
                    }
                }

                if !content.is_empty() {
                    let mut msg = Message::new(
                        Role::Assistant,
                        chrono::Utc::now().timestamp(),
                        content,
                    );

                    if let Some(id) = chunk.id {
                        msg = msg.with_id(id);
                    }

                    let final_usage = if chunk.choices[0].finish_reason.is_some() {
                        usage
                    } else {
                        None
                    };
                    usage_emitted |= final_usage.is_some();
                    yield (Some(msg), final_usage)
                } else if usage.is_some() {
                    usage_emitted = true;
                    yield (None, usage)
                }
            } else if usage.is_some() {
                usage_emitted = true;
                yield (None, usage)
            }
        }

        let filtered = think_filter.finish();
        let mut trailing_thinking = String::new();
        if !saw_structured_reasoning {
            trailing_thinking.push_str(&pending_inline_thinking);
            trailing_thinking.push_str(&filtered.thinking);
        }
        pending_inline_thinking.clear();

        if !filtered.content.is_empty() || !trailing_thinking.is_empty() {
            let mut content = Vec::new();

            if !filtered.content.is_empty() {
                content.push(MessageContentBlock::text(filtered.content));
            }

            if !trailing_thinking.is_empty() {
                content.push(MessageContentBlock::thinking(trailing_thinking, ""));
            }

            let mut message = Message::new(
                Role::Assistant,
                chrono::Utc::now().timestamp(),
                content,
            );
            if let Some(id) = last_response_id.clone() {
                message = message.with_id(id);
            }
            message.metadata.output_token_limit_reached =
                output_token_limit_reached && !output_token_limit_metadata_emitted;
            output_token_limit_metadata_emitted |= message.metadata.output_token_limit_reached;

            yield (Some(message), None)
        }

        if output_token_limit_reached && !output_token_limit_metadata_emitted {
            yield (Some(output_token_limit_marker(last_response_id.clone())), None)
        }

        if !usage_emitted && (last_response_id.is_some() || last_finish_reason.is_some()) {
            let mut usage = ProviderUsage::new(
                last_seen_model.unwrap_or_else(|| "unknown".to_string()),
                Usage::default(),
            );
            usage.response_id = last_response_id;
            usage.finish_reasons = last_finish_reason.map(|reason| vec![reason]);
            yield (None, Some(usage))
        }
    }
}

pub fn create_request(
    model_config: &ModelConfig,
    system: &str,
    messages: &[Message],
    tools: &[Tool],
    image_format: &ImageFormat,
    for_streaming: bool,
) -> anyhow::Result<Value, Error> {
    create_request_with_options(
        model_config,
        system,
        messages,
        tools,
        image_format,
        for_streaming,
        OpenAiFormatOptions {
            preserve_thinking_context: true,
            supports_vision: model_config.supports_vision.unwrap_or_default(),
            ..Default::default()
        },
    )
}

pub fn create_request_with_options(
    model_config: &ModelConfig,
    system: &str,
    messages: &[Message],
    tools: &[Tool],
    image_format: &ImageFormat,
    for_streaming: bool,
    format_options: OpenAiFormatOptions,
) -> anyhow::Result<Value, Error> {
    let (wire_model_name, _) = extract_reasoning_effort(&model_config.model_name);
    create_request_for_model_with_options(
        model_config,
        &wire_model_name,
        &model_config.model_name,
        system,
        messages,
        tools,
        image_format,
        for_streaming,
        format_options,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn create_request_for_model_with_options(
    model_config: &ModelConfig,
    wire_model_name: &str,
    capability_model_name: &str,
    system: &str,
    messages: &[Message],
    tools: &[Tool],
    image_format: &ImageFormat,
    for_streaming: bool,
    format_options: OpenAiFormatOptions,
) -> anyhow::Result<Value, Error> {
    if model_config.model_name.starts_with("o1-mini") {
        return Err(anyhow!(
            "o1-mini model is not currently supported since goose uses tool calling and o1-mini does not support it. Please use o1 or o3 models instead."
        ));
    }

    let (model_name, legacy_reasoning_effort) = extract_reasoning_effort(capability_model_name);
    let is_reasoning_model = is_openai_responses_model(&model_name);
    let supports_xai_effort = supports_xai_reasoning_effort(&model_name);
    let reasoning_effort = if is_reasoning_model {
        model_config
            .thinking_effort()
            .map_or(legacy_reasoning_effort, |effort| {
                openai_reasoning_effort_for_thinking(&model_name, effort)
            })
    } else if supports_xai_effort {
        model_config
            .thinking_effort()
            .and_then(|effort| xai_reasoning_effort_for_thinking(&model_name, effort))
    } else {
        None
    };

    let system_message = json!({
        "role": if is_reasoning_model { "developer" } else { "system" },
        "content": system
    });

    let messages_spec = format_messages_with_options(messages, image_format, format_options);
    let mut tools_spec = format_tools(tools)?;

    validate_tool_schemas(&mut tools_spec);

    let mut messages_array = vec![system_message];
    messages_array.extend(messages_spec);

    let mut payload = json!({
        "model": wire_model_name,
        "messages": messages_array
    });

    if let Some(effort) = reasoning_effort {
        payload["reasoning_effort"] = json!(effort);
    }

    if !tools_spec.is_empty() {
        payload["tools"] = json!(tools_spec);
    }

    if !is_reasoning_model && !supports_xai_effort {
        if let Some(temp) = model_config.temperature {
            payload["temperature"] = json!(temp);
        }
    }

    // Only emit max_tokens / max_completion_tokens when the user (via
    // GOOSE_MAX_TOKENS) or a canonical model record has supplied a value.
    // For unknown models on OpenAI-compatible endpoints (e.g. llama_swap,
    // lmstudio) sending the historic 4096 default truncates non-trivial
    // responses; omitting the field lets the server use its own max.
    if let Some(max_tokens) = model_config.max_tokens {
        let key = if is_reasoning_model {
            "max_completion_tokens"
        } else {
            "max_tokens"
        };
        payload
            .as_object_mut()
            .unwrap()
            .insert(key.to_string(), json!(max_tokens));
    }

    if for_streaming {
        payload["stream"] = json!(true);
        payload["stream_options"] = json!({"include_usage": true});
    }

    if let Some(params) = &model_config.request_params {
        if let Some(obj) = payload.as_object_mut() {
            for (key, value) in params {
                if !is_goose_internal_request_param(key) && !is_reserved_request_param_key(key) {
                    obj.insert(key.clone(), value.clone());
                }
            }
        }
    }

    Ok(payload)
}

/// Extract an explicit reasoning-effort suffix from a model name.
///
/// Returns `(base_model_name, Some(effort))` when the user appended a
/// recognised suffix like `-high` or `-xhigh`, e.g. `gpt-5.4-high` →
/// `("gpt-5.4", Some("high"))`.
///
/// When no suffix is present the effort is `None` — callers should omit
/// the `reasoning` field entirely so the API applies its own per-model
/// default. This avoids hard-coding a default that may be invalid for
/// certain models (e.g. `gpt-5-pro` only accepts `high`; older o-series
/// models reject `none` and `xhigh`).
pub fn extract_reasoning_effort(model_name: &str) -> (String, Option<String>) {
    if !is_openai_responses_model(model_name) {
        return (model_name.to_string(), None);
    }

    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"(?i)^(?P<base>.+)-(?P<effort>none|low|medium|high|xhigh)$").unwrap()
    });

    if let Some(captures) = re.captures(model_name) {
        let base = captures["base"].to_string();
        let effort = captures["effort"].to_ascii_lowercase();
        return (base, Some(effort));
    }

    (model_name.to_string(), None)
}

/// True when the model should use the OpenAI Responses API.
///
/// The Responses API is backwards-compatible with all OpenAI reasoning
/// models, so every `o`-series (`o1`, `o3`, `o4`, …), `gpt-5`, and `gpt-6` variant
/// routes here. The matcher intentionally scans the full model identifier so
/// hosted aliases like `databricks-gpt-5.4`, `goose-o3-mini`, or
/// `headless-goose-o3-mini` work without provider-specific normalization.
pub fn is_openai_responses_model(model_name: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"(?i)(?:^|[-/])(?:o\d+(?:$|-)|gpt-(?:5|6)(?:$|[-.]))").unwrap()
    });
    re.is_match(model_name)
}

/// Returns whether an xAI Chat Completions model accepts `reasoning_effort`.
pub fn supports_xai_reasoning_effort(model_name: &str) -> bool {
    let model_name = model_name.to_ascii_lowercase();

    model_name.starts_with("grok-4.5")
        || model_name.starts_with("grok-4.3")
        || model_name.starts_with("grok-3-mini")
}

/// Returns whether an xAI model performs server-side reasoning.
pub fn is_xai_reasoning_model(model_name: &str) -> bool {
    let model_name = model_name.to_ascii_lowercase();

    if model_name.contains("non-reasoning") || model_name.contains("non_reasoning") {
        return false;
    }

    supports_xai_reasoning_effort(&model_name)
        || model_name.starts_with("grok-4.20")
        || model_name.starts_with("grok-4-0709")
        || model_name.starts_with("grok-4-fast-reasoning")
        || model_name.starts_with("grok-4-1-fast-reasoning")
}

/// Maps Goose's effort levels to values accepted by xAI Chat Completions.
pub fn xai_reasoning_effort_for_thinking(
    model_name: &str,
    effort: ThinkingEffort,
) -> Option<String> {
    let model_name = model_name.to_ascii_lowercase();
    let supports_none = model_name.starts_with("grok-4.3");
    let supports_medium = !model_name.starts_with("grok-3-mini");

    match effort {
        ThinkingEffort::Off if supports_none => Some("none".to_string()),
        ThinkingEffort::Off => Some("low".to_string()),
        ThinkingEffort::Low => Some("low".to_string()),
        ThinkingEffort::Medium if supports_medium => Some("medium".to_string()),
        ThinkingEffort::Medium | ThinkingEffort::High | ThinkingEffort::Max => {
            Some("high".to_string())
        }
    }
}

pub fn openai_reasoning_effort_for_thinking(
    model_name: &str,
    effort: ThinkingEffort,
) -> Option<String> {
    let supported = openai_reasoning_efforts_for_model(model_name);

    let preferred: &[&str] = match effort {
        ThinkingEffort::Off => &["none", "low"],
        ThinkingEffort::Low => &["low", "medium", "high", "xhigh"],
        ThinkingEffort::Medium => &["medium", "high", "low", "xhigh"],
        ThinkingEffort::High => &["high", "medium", "xhigh", "low"],
        ThinkingEffort::Max => &["xhigh", "high", "medium", "low"],
    };

    preferred
        .iter()
        .find(|level| supported.contains(level))
        .map(|level| (*level).to_string())
}

pub(crate) fn openai_reasoning_efforts_for_model(model_name: &str) -> &'static [&'static str] {
    let normalized = model_name.to_ascii_lowercase();

    if normalized.contains("gpt-5") || normalized.contains("gpt-6") {
        if normalized.contains("-pro") || normalized.contains("/pro") {
            &["high"]
        } else if normalized.contains("gpt-5.4")
            || normalized.contains("gpt-5-4")
            || normalized.contains("gpt-5.5")
            || normalized.contains("gpt-5-5")
            || normalized.contains("gpt-5.6")
            || normalized.contains("gpt-5-6")
        {
            &["none", "low", "medium", "high", "xhigh"]
        } else {
            &["low", "medium", "high"]
        }
    } else {
        &["low", "medium", "high"]
    }
}

const MAX_FUNCTION_NAME_LENGTH: usize = 128;

pub fn sanitize_function_name(name: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"[^a-zA-Z0-9_-]").unwrap());
    re.replace_all(name, "_")
        .chars()
        .take(MAX_FUNCTION_NAME_LENGTH)
        .collect()
}

pub fn is_valid_function_name(name: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"^[a-zA-Z0-9_-]+$").unwrap());
    re.is_match(name)
}

#[cfg(test)]
mod document_tests {
    use super::*;

    fn format(messages: &[Message]) -> Vec<Value> {
        format_messages_with_options(
            messages,
            &ImageFormat::OpenAi,
            OpenAiFormatOptions {
                supports_vision: true,
                ..Default::default()
            },
        )
    }

    #[test]
    fn user_document_becomes_a_file_content_part() {
        let spec = format(&[Message::user().with_document(
            "cGRmLWJ5dGVz",
            "application/pdf",
            Some("q3-report.pdf".to_string()),
        )]);

        assert_eq!(spec.len(), 1);
        assert_eq!(
            spec[0]["content"][0],
            json!({
                "type": "file",
                "file": {
                    "filename": "q3-report.pdf",
                    "file_data": "data:application/pdf;base64,cGRmLWJ5dGVz",
                }
            })
        );
    }

    #[test]
    fn assistant_document_becomes_a_text_part() {
        let spec = format(&[Message::assistant().with_document(
            "cGRmLWJ5dGVz",
            "application/pdf",
            Some("q3-report.pdf".to_string()),
        )]);

        assert_eq!(spec.len(), 1);
        assert_eq!(spec[0]["role"], "assistant");
        let text = spec[0]["content"].as_str().unwrap();
        assert!(text.contains("q3-report.pdf"), "{text}");
        assert!(text.contains("user messages"), "{text}");
        assert!(!text.contains("cGRmLWJ5dGVz"), "{text}");
    }

    #[test]
    fn unsupported_document_media_type_becomes_an_explicit_text_part() {
        let spec = format(&[Message::user().with_document(
            "cm93cw==",
            "text/csv",
            Some("rows.csv".to_string()),
        )]);

        let text = spec[0]["content"].as_str().unwrap();
        assert!(text.contains("rows.csv"), "{text}");
        assert!(text.contains("application/pdf"), "{text}");
        assert!(!text.contains("cm93cw=="), "{text}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::message::Message;
    use rmcp::model::CallToolResult;
    use rmcp::object;
    use serde_json::json;
    use test_case::test_case;
    use tokio::pin;
    use tokio_stream::{self, StreamExt};

    fn test_model_config(model_name: &str) -> ModelConfig {
        ModelConfig::new(model_name)
    }

    #[test]
    fn test_validate_tool_schemas() {
        // Test case 1: Empty parameters object
        // Input JSON with an incomplete parameters object
        let mut actual = vec![json!({
            "type": "function",
            "function": {
                "name": "test_func",
                "description": "test description",
                "parameters": {
                    "type": "object"
                }
            }
        })];

        // Run the function to validate and update schemas
        validate_tool_schemas(&mut actual);

        // Expected JSON after validation
        let expected = vec![json!({
            "type": "function",
            "function": {
                "name": "test_func",
                "description": "test description",
                "parameters": {
                    "type": "object",
                    "properties": {},
                    "required": []
                }
            }
        })];

        // Compare entire JSON structures instead of individual fields
        assert_eq!(actual, expected);

        // Test case 2: Missing type field
        let mut tools = vec![json!({
            "type": "function",
            "function": {
                "name": "test_func",
                "description": "test description",
                "parameters": {
                    "properties": {}
                }
            }
        })];

        validate_tool_schemas(&mut tools);

        let params = tools[0]["function"]["parameters"].as_object().unwrap();
        assert_eq!(params["type"], "object");

        // Test case 3: Complete valid schema should remain unchanged
        let original_schema = json!({
            "type": "function",
            "function": {
                "name": "test_func",
                "description": "test description",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "location": {
                            "type": "string",
                            "description": "City and country"
                        }
                    },
                    "required": ["location"]
                }
            }
        });

        let mut tools = vec![original_schema.clone()];
        validate_tool_schemas(&mut tools);
        assert_eq!(tools[0], original_schema);

        // Test case 4: anyOf nullable is unwrapped, preserving sibling metadata
        let mut tools = vec![json!({
            "type": "function",
            "function": {
                "name": "shell",
                "description": "run shell",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": { "type": "string" },
                        "timeout_secs": {
                            "description": "timeout in seconds",
                            "anyOf": [
                                { "type": "integer", "format": "uint64", "minimum": 0 },
                                { "type": "null" }
                            ]
                        }
                    },
                    "required": ["command"]
                }
            }
        })];
        validate_tool_schemas(&mut tools);
        let timeout_schema = &tools[0]["function"]["parameters"]["properties"]["timeout_secs"];
        assert_eq!(timeout_schema["type"], "integer");
        assert_eq!(timeout_schema["format"], "uint64");
        assert_eq!(timeout_schema["description"], "timeout in seconds");
        assert!(timeout_schema.get("anyOf").is_none());

        // Test case 4b: type array form (schemars 1.x style for nullable primitives)
        let mut tools = vec![json!({
            "type": "function",
            "function": {
                "name": "shell",
                "description": "run shell",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": { "type": "string" },
                        "timeout_secs": {
                            "type": ["integer", "null"],
                            "format": "uint64",
                            "minimum": 0
                        }
                    },
                    "required": ["command"]
                }
            }
        })];
        validate_tool_schemas(&mut tools);
        let timeout_schema = &tools[0]["function"]["parameters"]["properties"]["timeout_secs"];
        assert_eq!(timeout_schema["type"], "integer");
        assert!(!timeout_schema["type"].is_array());
    }

    #[test]
    fn test_validate_tool_schemas_sanitizes_defs() {
        let mut tools = vec![json!({
            "type": "function",
            "function": {
                "name": "cache",
                "description": "manage cache",
                "parameters": {
                    "type": "object",
                    "$defs": {
                        "CacheCommand": {
                            "oneOf": [
                                { "description": "List cached files", "type": "string", "const": "list" },
                                { "description": "Clear cached files", "type": "string", "const": "clear" }
                            ]
                        },
                        "TextStyle": {
                            "type": "object",
                            "properties": {
                                "size": { "type": ["integer", "null"], "format": "int32" }
                            }
                        }
                    },
                    "properties": {
                        "command": { "$ref": "#/$defs/CacheCommand" },
                        "style": { "$ref": "#/$defs/TextStyle" }
                    },
                    "required": ["command"]
                }
            }
        })];
        validate_tool_schemas(&mut tools);
        let defs = &tools[0]["function"]["parameters"]["$defs"];
        assert!(defs["CacheCommand"].get("oneOf").is_none());
        assert_eq!(defs["CacheCommand"]["anyOf"].as_array().unwrap().len(), 2);
        assert_eq!(defs["TextStyle"]["properties"]["size"]["type"], "integer");
    }

    const OPENAI_TOOL_USE_RESPONSE: &str = r#"{
        "choices": [{
            "role": "assistant",
            "message": {
                "tool_calls": [{
                    "id": "1",
                    "function": {
                        "name": "example_fn",
                        "arguments": "{\"param\": \"value\"}"
                    }
                }]
            }
        }],
        "usage": {
            "input_tokens": 10,
            "output_tokens": 25,
            "total_tokens": 35
        }
    }"#;

    #[test]
    fn test_format_messages() -> anyhow::Result<()> {
        let message = Message::user().with_text("Hello");
        let spec = format_messages(&[message], &ImageFormat::OpenAi);

        assert_eq!(spec.len(), 1);
        assert_eq!(spec[0]["role"], "user");
        assert_eq!(spec[0]["content"], "Hello");
        Ok(())
    }

    #[test]
    fn test_format_tools() -> anyhow::Result<()> {
        let tool = Tool::new(
            "test_tool",
            "A test tool",
            object!({
                "type": "object",
                "properties": {
                    "input": {
                        "type": "string",
                        "description": "Test parameter"
                    }
                },
                "required": ["input"]
            }),
        );

        let spec = format_tools(&[tool])?;

        assert_eq!(spec.len(), 1);
        assert_eq!(spec[0]["type"], "function");
        assert_eq!(spec[0]["function"]["name"], "test_tool");
        Ok(())
    }

    #[test]
    fn test_format_messages_complex() -> anyhow::Result<()> {
        let mut messages = vec![
            Message::assistant().with_text("Hello!"),
            Message::user().with_text("How are you?"),
            Message::assistant().with_tool_request(
                "tool1",
                Ok(CallToolRequestParams::new("example")
                    .with_arguments(object!({"param1": "value1"}))),
            ),
        ];

        // Get the ID from the tool request to use in the response
        let tool_id = if let MessageContentBlock::ToolRequest(request) = &messages[2].content[0] {
            request.id.clone()
        } else {
            panic!("should be tool request");
        };

        messages.push(Message::user().with_tool_response(
            tool_id,
            Ok(CallToolResult::success(vec![ContentBlock::text("Result")])),
        ));

        let spec = format_messages(&messages, &ImageFormat::OpenAi);

        assert_eq!(spec.len(), 4);
        assert_eq!(spec[0]["role"], "assistant");
        assert_eq!(spec[0]["content"], "Hello!");
        assert_eq!(spec[1]["role"], "user");
        assert_eq!(spec[1]["content"], "How are you?");
        assert_eq!(spec[2]["role"], "assistant");
        assert!(spec[2]["tool_calls"].is_array());
        assert_eq!(spec[3]["role"], "tool");
        assert_eq!(spec[3]["content"], "Result");
        assert_eq!(spec[3]["tool_call_id"], spec[2]["tool_calls"][0]["id"]);

        Ok(())
    }

    #[test]
    fn test_format_messages_multiple_content() -> anyhow::Result<()> {
        let mut messages = vec![Message::assistant().with_tool_request(
            "tool1",
            Ok(CallToolRequestParams::new("example").with_arguments(object!({"param1": "value1"}))),
        )];

        // Get the ID from the tool request to use in the response
        let tool_id = if let MessageContentBlock::ToolRequest(request) = &messages[0].content[0] {
            request.id.clone()
        } else {
            panic!("should be tool request");
        };

        messages.push(Message::user().with_tool_response(
            tool_id,
            Ok(CallToolResult::success(vec![ContentBlock::text("Result")])),
        ));

        let spec = format_messages(&messages, &ImageFormat::OpenAi);

        assert_eq!(spec.len(), 2);
        assert_eq!(spec[0]["role"], "assistant");
        assert!(spec[0]["tool_calls"].is_array());
        assert_eq!(spec[1]["role"], "tool");
        assert_eq!(spec[1]["content"], "Result");
        assert_eq!(spec[1]["tool_call_id"], spec[0]["tool_calls"][0]["id"]);

        Ok(())
    }

    #[test]
    fn test_format_tools_duplicate() -> anyhow::Result<()> {
        let tool1 = Tool::new(
            "test_tool",
            "Test tool",
            object!({
                "type": "object",
                "properties": {
                    "input": {
                        "type": "string",
                        "description": "Test parameter"
                    }
                },
                "required": ["input"]
            }),
        );

        let tool2 = Tool::new(
            "test_tool",
            "Test tool",
            object!({
                "type": "object",
                "properties": {
                    "input": {
                        "type": "string",
                        "description": "Test parameter"
                    }
                },
                "required": ["input"]
            }),
        );

        let result = format_tools(&[tool1, tool2]);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Duplicate tool name"));

        Ok(())
    }

    #[test]
    fn test_format_tools_empty() -> anyhow::Result<()> {
        let spec = format_tools(&[])?;
        assert!(spec.is_empty());
        Ok(())
    }

    #[test]
    fn test_format_messages_with_image_path() -> anyhow::Result<()> {
        // Create a temporary PNG file with valid PNG magic numbers
        let temp_dir = tempfile::tempdir()?;
        let png_path = temp_dir.path().join("test.png");
        let png_data = [
            0x89, 0x50, 0x4E, 0x47, // PNG magic number
            0x0D, 0x0A, 0x1A, 0x0A, // PNG header
            0x00, 0x00, 0x00, 0x0D, // Rest of fake PNG data
        ];
        std::fs::write(&png_path, png_data)?;
        let png_path_str = png_path.to_str().unwrap();

        // Create user message with image path - should load the image when vision is supported
        let user_message = Message::user().with_text(format!("Here is an image: {}", png_path_str));
        let spec = format_messages_with_options(
            &[user_message],
            &ImageFormat::OpenAi,
            OpenAiFormatOptions {
                preserve_thinking_context: true,
                supports_vision: true,
                ..Default::default()
            },
        );

        assert_eq!(spec.len(), 1);
        assert_eq!(spec[0]["role"], "user");

        // Content should be an array with text and image
        let content = spec[0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["type"], "text");
        assert!(content[0]["text"].as_str().unwrap().contains(png_path_str));
        assert_eq!(content[1]["type"], "image_url");
        assert!(content[1]["image_url"]["url"]
            .as_str()
            .unwrap()
            .starts_with("data:image/png;base64,"));

        // Create assistant message with same text - should NOT load the image
        let assistant_message =
            Message::assistant().with_text(format!("I saved the output to {}", png_path_str));
        let spec = format_messages_with_options(
            &[assistant_message],
            &ImageFormat::OpenAi,
            OpenAiFormatOptions {
                preserve_thinking_context: true,
                supports_vision: true,
                ..Default::default()
            },
        );

        assert_eq!(spec.len(), 1);
        assert_eq!(spec[0]["role"], "assistant");

        // Content should be plain text, NOT an array with image
        let content = spec[0]["content"].as_str();
        assert!(
            content.is_some(),
            "Assistant message content should be a string, not an array with image"
        );
        assert!(content.unwrap().contains(png_path_str));

        Ok(())
    }

    #[test]
    fn test_format_messages_with_image_path_passthrough_when_not_vision() -> anyhow::Result<()> {
        // Create a temporary PNG file with valid PNG magic numbers
        let temp_dir = tempfile::tempdir()?;
        let png_path = temp_dir.path().join("test.png");
        let png_data = [
            0x89, 0x50, 0x4E, 0x47, // PNG magic number
            0x0D, 0x0A, 0x1A, 0x0A, // PNG header
            0x00, 0x00, 0x00, 0x0D, // Rest of fake PNG data
        ];
        std::fs::write(&png_path, png_data)?;
        let png_path_str = png_path.to_str().unwrap();

        // User message with image path: with vision NOT supported, the path must
        // pass through as plain text (a non-vision model can forward it to a
        // vision subagent instead of 400ing on an injected image_url block).
        let user_message = Message::user().with_text(format!("Here is an image: {}", png_path_str));

        let spec = format_messages_with_options(
            std::slice::from_ref(&user_message),
            &ImageFormat::OpenAi,
            OpenAiFormatOptions {
                preserve_thinking_context: true,
                supports_vision: false,
                ..Default::default()
            },
        );
        assert_eq!(spec.len(), 1);
        assert_eq!(spec[0]["role"], "user");
        // Single text block collapses to a plain string — the path survives verbatim.
        let content = spec[0]["content"].as_str().unwrap();
        assert!(content.contains(png_path_str));
        assert!(!content.contains("image_url"));
        assert!(!content.contains("data:image"));

        // Default options (bare format_messages wrapper) must behave the same:
        // unaffirmed vision -> passthrough.
        let spec = format_messages(&[user_message], &ImageFormat::OpenAi);
        let content = spec[0]["content"].as_str().unwrap();
        assert!(content.contains(png_path_str));
        assert!(!content.contains("image_url"));

        Ok(())
    }

    #[test]
    fn test_create_request_threads_supports_vision() -> anyhow::Result<()> {
        // Create a temporary PNG file with valid PNG magic numbers
        let temp_dir = tempfile::tempdir()?;
        let png_path = temp_dir.path().join("test.png");
        let png_data = [
            0x89, 0x50, 0x4E, 0x47, // PNG magic number
            0x0D, 0x0A, 0x1A, 0x0A, // PNG header
            0x00, 0x00, 0x00, 0x0D, // Rest of fake PNG data
        ];
        std::fs::write(&png_path, png_data)?;
        let png_path_str = png_path.to_str().unwrap();
        let message = Message::user().with_text(format!("Here is an image: {}", png_path_str));

        // Vision affirmed: path is converted to an image_url block.
        let vision = ModelConfig::new("gpt-4o").with_vision_support(true);
        let request = create_request(
            &vision,
            "system",
            std::slice::from_ref(&message),
            &[],
            &ImageFormat::OpenAi,
            false,
        )?;
        let messages = request["messages"].as_array().unwrap();
        let content = messages[1]["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[1]["type"], "image_url");

        // Unknown (None): passthrough, no image_url.
        let unknown = ModelConfig::new("gpt-4o");
        let request = create_request(
            &unknown,
            "system",
            std::slice::from_ref(&message),
            &[],
            &ImageFormat::OpenAi,
            false,
        )?;
        let messages = request["messages"].as_array().unwrap();
        let content = messages[1]["content"].as_str().unwrap();
        assert!(content.contains(png_path_str));
        assert!(!content.contains("image_url"));

        // Explicitly non-vision: passthrough, no image_url.
        let non_vision = ModelConfig::new("gpt-4o").with_vision_support(false);
        let request = create_request(
            &non_vision,
            "system",
            &[message],
            &[],
            &ImageFormat::OpenAi,
            false,
        )?;
        let messages = request["messages"].as_array().unwrap();
        let content = messages[1]["content"].as_str().unwrap();
        assert!(!content.contains("image_url"));

        Ok(())
    }

    #[test]
    fn test_format_messages_with_image_block_passthrough_when_not_vision() -> anyhow::Result<()> {
        let user_message = Message::user().with_image("aW1hZ2VkYXRh", "image/png");

        // Non-vision: explicit image content is replaced with a text placeholder
        // at format time — session history is untouched, so a vision model (or a
        // delegated vision subagent) can still see the real image later.
        let spec = format_messages_with_options(
            std::slice::from_ref(&user_message),
            &ImageFormat::OpenAi,
            OpenAiFormatOptions {
                preserve_thinking_context: true,
                supports_vision: false,
                ..Default::default()
            },
        );
        assert_eq!(spec.len(), 1);
        let content = spec[0]["content"].as_str().unwrap();
        assert_eq!(content, "[image omitted: model does not support vision]");
        assert!(!content.contains("image_url"));
        assert!(!content.contains("data:image"));

        // Vision: the image is converted to an image_url block (existing behavior).
        let spec = format_messages_with_options(
            &[user_message],
            &ImageFormat::OpenAi,
            OpenAiFormatOptions {
                preserve_thinking_context: true,
                supports_vision: true,
                ..Default::default()
            },
        );
        let content = spec[0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "image_url");
        assert!(content[0]["image_url"]["url"]
            .as_str()
            .unwrap()
            .starts_with("data:image/png;base64,"));

        Ok(())
    }

    #[test]
    fn test_tool_response_image_omitted_when_not_vision() -> anyhow::Result<()> {
        let tool_response = Message::user().with_tool_response(
            "tool1",
            Ok(rmcp::model::CallToolResult::success(vec![
                rmcp::model::ContentBlock::image("aW1hZ2VkYXRh", "image/png"),
            ])),
        );

        // Non-vision: the separate user image message is NOT emitted — this is
        // what un-bricks sessions (a converted image in the next request 400s).
        let spec = format_messages_with_options(
            std::slice::from_ref(&tool_response),
            &ImageFormat::OpenAi,
            OpenAiFormatOptions {
                preserve_thinking_context: true,
                supports_vision: false,
                ..Default::default()
            },
        );
        let serialized = serde_json::to_value(&spec).unwrap().to_string();
        assert!(!serialized.contains("image_url"));
        assert!(serialized.contains(
            "This tool result included an image that was omitted as the model does not support vision."
        ));

        // Vision: the separate user image message IS emitted (existing behavior).
        let spec = format_messages_with_options(
            &[tool_response],
            &ImageFormat::OpenAi,
            OpenAiFormatOptions {
                preserve_thinking_context: true,
                supports_vision: true,
                ..Default::default()
            },
        );
        let serialized = serde_json::to_value(&spec).unwrap().to_string();
        assert!(serialized.contains("image_url"));
        assert!(serialized
            .contains("This tool result included an image that is uploaded in the next message."));

        Ok(())
    }

    #[test]
    fn test_format_messages_with_text_and_image_preserves_order() {
        // Text before image: order should be [text, image]
        let msg_text_first = Message::user()
            .with_text("Describe this image")
            .with_image("aW1hZ2VkYXRh", "image/png");

        let spec = format_messages_with_options(
            &[msg_text_first],
            &ImageFormat::OpenAi,
            OpenAiFormatOptions {
                preserve_thinking_context: true,
                supports_vision: true,
                ..Default::default()
            },
        );
        assert_eq!(spec.len(), 1);
        assert_eq!(spec[0]["role"], "user");

        let content = spec[0]["content"]
            .as_array()
            .expect("content should be an array");
        assert_eq!(content.len(), 2, "expected text + image entries");
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "Describe this image");
        assert_eq!(content[1]["type"], "image_url");

        // Image before text: order should be [image, text]
        let msg_image_first = Message::user()
            .with_image("aW1hZ2VkYXRh", "image/png")
            .with_text("What do you see?");

        let spec2 = format_messages_with_options(
            &[msg_image_first],
            &ImageFormat::OpenAi,
            OpenAiFormatOptions {
                preserve_thinking_context: true,
                supports_vision: true,
                ..Default::default()
            },
        );
        let content2 = spec2[0]["content"]
            .as_array()
            .expect("content should be an array");
        assert_eq!(content2.len(), 2, "expected image + text entries");
        assert_eq!(content2[0]["type"], "image_url");
        assert_eq!(content2[1]["type"], "text");
        assert_eq!(content2[1]["text"], "What do you see?");
    }

    #[test]
    fn test_response_to_message_text() -> anyhow::Result<()> {
        let response = json!({
            "choices": [{
                "role": "assistant",
                "message": {
                    "content": "Hello from John Cena!"
                }
            }],
            "usage": {
                "input_tokens": 10,
                "output_tokens": 25,
                "total_tokens": 35
            }
        });

        let message = response_to_message(&response)?;
        assert_eq!(message.content.len(), 1);
        if let MessageContentBlock::Text(text) = &message.content[0] {
            assert_eq!(text.text, "Hello from John Cena!");
        } else {
            panic!("Expected Text content");
        }
        assert!(matches!(message.role, Role::Assistant));

        Ok(())
    }

    #[test]
    fn test_record_response_metadata() {
        let response = json!({
            "id": "chatcmpl-123",
            "choices": [
                {"finish_reason": "stop"},
                {"finish_reason": "tool_calls"}
            ]
        });
        let mut usage = ProviderUsage::new("test-model".to_string(), Usage::default());

        record_response_metadata(&mut usage, &response);

        assert_eq!(usage.response_id.as_deref(), Some("chatcmpl-123"));
        assert_eq!(
            usage.finish_reasons,
            Some(vec!["stop".to_string(), "tool_calls".to_string()])
        );
    }

    #[test]
    fn test_response_to_message_marks_length_finish_reason() -> anyhow::Result<()> {
        let response = json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "Partial answer"
                },
                "finish_reason": "length"
            }]
        });

        let message = response_to_message(&response)?;

        assert_eq!(message.as_concat_text(), "Partial answer");
        assert!(message.metadata.output_token_limit_reached);

        Ok(())
    }

    #[test]
    fn test_response_to_message_rejects_length_terminated_tool_calls() -> anyhow::Result<()> {
        let response = json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [
                        {
                            "id": "call_empty",
                            "type": "function",
                            "function": {
                                "name": "empty_tool",
                                "arguments": ""
                            }
                        },
                        {
                            "id": "call_valid",
                            "type": "function",
                            "function": {
                                "name": "valid_tool",
                                "arguments": "{\"value\":true}"
                            }
                        }
                    ]
                },
                "finish_reason": "length"
            }]
        });

        let message = response_to_message(&response)?;

        assert!(message.metadata.output_token_limit_reached);
        assert_eq!(message.content.len(), 2);
        for (content, expected_id) in message.content.iter().zip(["call_empty", "call_valid"]) {
            let MessageContentBlock::ToolRequest(request) = content else {
                panic!("expected tool request");
            };
            assert_eq!(request.id, expected_id);
            let error = request
                .tool_call
                .as_ref()
                .expect_err("length-terminated tool call must not be executable");
            assert_eq!(error.code, ErrorCode::INVALID_PARAMS);
            assert!(error.message.contains("output token limit"));
        }

        Ok(())
    }

    #[test]
    fn test_response_to_message_valid_toolrequest() -> anyhow::Result<()> {
        let response: Value = serde_json::from_str(OPENAI_TOOL_USE_RESPONSE)?;
        let message = response_to_message(&response)?;

        assert_eq!(message.content.len(), 1);
        if let MessageContentBlock::ToolRequest(request) = &message.content[0] {
            let tool_call = request.tool_call.as_ref().unwrap();
            assert_eq!(tool_call.name, "example_fn");
            assert_eq!(tool_call.arguments, Some(object!({"param": "value"})));
        } else {
            panic!("Expected ToolRequest content");
        }

        Ok(())
    }

    #[test]
    fn test_response_to_message_empty_func_name() -> anyhow::Result<()> {
        let mut response: Value = serde_json::from_str(OPENAI_TOOL_USE_RESPONSE)?;
        response["choices"][0]["message"]["tool_calls"][0]["function"]["name"] = json!("");

        let message = response_to_message(&response)?;

        if let MessageContentBlock::ToolRequest(request) = &message.content[0] {
            match &request.tool_call {
                Err(ErrorData {
                    code: ErrorCode::INVALID_REQUEST,
                    message: msg,
                    data: None,
                }) => {
                    assert!(msg.starts_with("The provided function name"));
                }
                _ => panic!("Expected invalid-request error for empty name"),
            }
        } else {
            panic!("Expected ToolRequest content");
        }

        Ok(())
    }

    #[test]
    fn test_response_to_message_passes_names_through_to_dispatch() -> anyhow::Result<()> {
        for name in [
            "developer.shell",
            "functions.example_fn",
            "example fn",
            "???!",
        ] {
            let mut response: Value = serde_json::from_str(OPENAI_TOOL_USE_RESPONSE)?;
            response["choices"][0]["message"]["tool_calls"][0]["function"]["name"] = json!(name);

            let message = response_to_message(&response)?;

            if let MessageContentBlock::ToolRequest(request) = &message.content[0] {
                let tool_call = request.tool_call.as_ref().expect("tool call should parse");
                assert_eq!(tool_call.name, name, "name must pass through verbatim");
            } else {
                panic!("Expected ToolRequest content");
            }
        }

        Ok(())
    }

    #[test]
    fn test_response_to_message_fenced_arguments() -> anyhow::Result<()> {
        let mut response: Value = serde_json::from_str(OPENAI_TOOL_USE_RESPONSE)?;
        response["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"] =
            json!("```json\n{\"param\": \"value\"}\n```");

        let message = response_to_message(&response)?;

        if let MessageContentBlock::ToolRequest(request) = &message.content[0] {
            let tool_call = request.tool_call.as_ref().expect("tool call should parse");
            assert_eq!(tool_call.arguments, Some(object!({"param": "value"})));
        } else {
            panic!("Expected ToolRequest content");
        }

        Ok(())
    }

    #[test]
    fn test_response_to_message_json_decode_error() -> anyhow::Result<()> {
        let mut response: Value = serde_json::from_str(OPENAI_TOOL_USE_RESPONSE)?;
        response["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"] =
            json!("invalid json {");

        let message = response_to_message(&response)?;

        if let MessageContentBlock::ToolRequest(request) = &message.content[0] {
            match &request.tool_call {
                Err(ErrorData {
                    code: ErrorCode::INVALID_PARAMS,
                    message: msg,
                    data: None,
                }) => {
                    assert!(msg.contains("tool arguments") || msg.contains("truncated"));
                }
                _ => panic!("Expected InvalidParameters error"),
            }
        } else {
            panic!("Expected ToolRequest content");
        }

        Ok(())
    }

    #[test]
    fn test_response_to_message_non_object_arguments() -> anyhow::Result<()> {
        // Weaker models sometimes emit tool arguments that are valid JSON but
        // not an object (here, a bare array). This must surface as a tool error,
        // NOT panic via rmcp's `object()` debug-assert.
        let mut response: Value = serde_json::from_str(OPENAI_TOOL_USE_RESPONSE)?;
        response["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"] =
            json!("[1, 2, 3]");

        let message = response_to_message(&response)?;

        if let MessageContentBlock::ToolRequest(request) = &message.content[0] {
            match &request.tool_call {
                Err(ErrorData {
                    code: ErrorCode::INVALID_PARAMS,
                    message: msg,
                    data: None,
                }) => {
                    assert!(msg.contains("must be a JSON object"));
                    assert!(msg.contains("an array"));
                    assert!(
                        msg.contains("example_fn"),
                        "error must name the original tool so the model can retry it: {msg}"
                    );
                }
                _ => panic!("Expected InvalidParameters error for non-object args"),
            }
        } else {
            panic!("Expected ToolRequest content");
        }

        Ok(())
    }

    #[test]
    fn test_response_to_message_empty_argument() -> anyhow::Result<()> {
        let mut response: Value = serde_json::from_str(OPENAI_TOOL_USE_RESPONSE)?;
        response["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"] =
            serde_json::Value::String("".to_string());

        let message = response_to_message(&response)?;

        if let MessageContentBlock::ToolRequest(request) = &message.content[0] {
            let tool_call = request.tool_call.as_ref().unwrap();
            assert_eq!(tool_call.name, "example_fn");
            assert_eq!(tool_call.arguments, Some(object!({})));
        } else {
            panic!("Expected ToolRequest content");
        }

        Ok(())
    }

    #[test]
    fn test_response_to_message_api_error() -> anyhow::Result<()> {
        // Test that API responses with an "error" field return the error message
        let response = json!({
            "error": {
                "message": "You have exceeded your quota",
                "type": "insufficient_quota",
                "code": "quota_exceeded"
            }
        });

        let result = response_to_message(&response);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("API error:"));
        assert!(err.to_string().contains("You have exceeded your quota"));

        Ok(())
    }

    #[test]
    fn test_response_to_message_api_error_unknown() -> anyhow::Result<()> {
        // Test that API responses with an "error" field but no message return "Unknown error"
        let response = json!({
            "error": {
                "type": "some_error"
            }
        });

        let result = response_to_message(&response);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("API error:"));
        assert!(err.to_string().contains("Unknown error"));

        Ok(())
    }

    #[test]
    fn test_response_to_message_no_choices() -> anyhow::Result<()> {
        // Test that responses without "choices" return an error
        let response = json!({
            "id": "chatcmpl-123",
            "object": "chat.completion",
            "created": 1234567890
        });

        let result = response_to_message(&response);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("No message in API response"));

        Ok(())
    }

    #[test]
    fn test_format_messages_tool_request_with_none_arguments() -> anyhow::Result<()> {
        // Test that tool calls with None arguments are formatted as "{}" string
        let message = Message::assistant()
            .with_tool_request("tool1", Ok(CallToolRequestParams::new("test_tool")));

        let spec = format_messages(&[message], &ImageFormat::OpenAi);

        assert_eq!(spec.len(), 1);
        assert_eq!(spec[0]["role"], "assistant");
        assert!(spec[0]["tool_calls"].is_array());

        let tool_call = &spec[0]["tool_calls"][0];
        assert_eq!(tool_call["id"], "tool1");
        assert_eq!(tool_call["type"], "function");
        assert_eq!(tool_call["function"]["name"], "test_tool");
        // This should be the string "{}", not null
        assert_eq!(tool_call["function"]["arguments"], "{}");

        Ok(())
    }

    #[test]
    fn test_format_messages_tool_request_with_some_arguments() -> anyhow::Result<()> {
        // Test that tool calls with Some arguments are properly JSON-serialized
        let message = Message::assistant().with_tool_request(
            "tool1",
            Ok(CallToolRequestParams::new("test_tool")
                .with_arguments(object!({"param": "value", "number": 42}))),
        );

        let spec = format_messages(&[message], &ImageFormat::OpenAi);

        assert_eq!(spec.len(), 1);
        assert_eq!(spec[0]["role"], "assistant");
        assert!(spec[0]["tool_calls"].is_array());

        let tool_call = &spec[0]["tool_calls"][0];
        assert_eq!(tool_call["id"], "tool1");
        assert_eq!(tool_call["type"], "function");
        assert_eq!(tool_call["function"]["name"], "test_tool");
        // This should be a JSON string representation
        let args_str = tool_call["function"]["arguments"].as_str().unwrap();
        let parsed_args: Value = serde_json::from_str(args_str)?;
        assert_eq!(parsed_args["param"], "value");
        assert_eq!(parsed_args["number"], 42);

        Ok(())
    }

    #[test]
    fn test_format_messages_multiple_text_blocks() -> anyhow::Result<()> {
        let message = Message::user()
            .with_text("--- Resource: file:///test.md ---\n# Test\n\n---\n")
            .with_text(" What is in the file?");

        let spec = format_messages(&[message], &ImageFormat::OpenAi);

        assert_eq!(spec.len(), 1);
        assert_eq!(spec[0]["role"], "user");
        assert_eq!(
            spec[0]["content"],
            "--- Resource: file:///test.md ---\n# Test\n\n---\n\n What is in the file?"
        );
        Ok(())
    }

    #[test]
    fn test_create_request_gpt_4o() -> anyhow::Result<()> {
        // Test default medium reasoning effort for O3 model
        let model_config = test_model_config("gpt-4o").with_max_tokens(Some(1024));
        let request = create_request(
            &model_config,
            "system",
            &[],
            &[],
            &ImageFormat::OpenAi,
            false,
        )?;
        let obj = request.as_object().unwrap();
        let expected = json!({
            "model": "gpt-4o",
            "messages": [
                {
                    "role": "system",
                    "content": "system"
                }
            ],
            "max_tokens": 1024
        });

        for (key, value) in expected.as_object().unwrap() {
            assert_eq!(obj.get(key).unwrap(), value);
        }

        Ok(())
    }

    #[test]
    fn test_create_request_omits_max_tokens_when_unset() -> anyhow::Result<()> {
        // Unknown models on OpenAI-compatible local providers (llama_swap,
        // lmstudio) have no canonical record and no GOOSE_MAX_TOKENS, so the
        // request must not pin the legacy 4096 default. See issue #9007.
        let model_config = test_model_config("some-unknown-local-model");
        let request = create_request(
            &model_config,
            "system",
            &[],
            &[],
            &ImageFormat::OpenAi,
            false,
        )?;
        let obj = request.as_object().unwrap();
        assert!(
            !obj.contains_key("max_tokens"),
            "max_tokens should be omitted when model_config.max_tokens is None"
        );
        assert!(
            !obj.contains_key("max_completion_tokens"),
            "max_completion_tokens should be omitted when model_config.max_tokens is None"
        );
        Ok(())
    }

    #[test]
    fn test_request_params_preserve_reserved_fields() -> anyhow::Result<()> {
        let params = std::collections::HashMap::from([
            (
                "thinking".to_string(),
                json!({
                    "type": "enabled",
                    "clear_thinking": false
                }),
            ),
            ("stream".to_string(), json!(false)),
            (
                "stream_options".to_string(),
                json!({"include_usage": false}),
            ),
            ("model".to_string(), json!("wrong-model")),
            ("messages".to_string(), json!([])),
            ("max_tokens".to_string(), json!(1)),
            ("temperature".to_string(), json!(2.0)),
            ("provider_custom".to_string(), json!("allowed")),
            ("thinking_effort".to_string(), json!("high")),
            ("disable_prompt_cache".to_string(), json!(true)),
            ("preserve_thinking_context".to_string(), json!(true)),
        ]);
        let model_config = test_model_config("glm-4.7")
            .with_max_tokens(Some(4096))
            .with_merged_request_params(params);

        let request = create_request(
            &model_config,
            "system",
            &[],
            &[],
            &ImageFormat::OpenAi,
            true,
        )?;

        assert_eq!(
            request["thinking"],
            json!({
                "type": "enabled",
                "clear_thinking": false
            })
        );
        assert_eq!(request["stream"], true);
        assert_eq!(request["stream_options"], json!({"include_usage": true}));
        assert_eq!(request["model"], "glm-4.7");
        assert_eq!(request["messages"][0]["role"], "system");
        assert_eq!(request["max_tokens"], 1);
        assert_eq!(request["temperature"], 2.0);
        assert_eq!(request["provider_custom"], "allowed");
        assert!(request.get("thinking_effort").is_none());
        assert!(request.get("disable_prompt_cache").is_none());
        assert!(request.get("preserve_thinking_context").is_none());

        Ok(())
    }

    #[test]
    fn test_create_request_o1_default() -> anyhow::Result<()> {
        let model_config = test_model_config("o1").with_max_tokens(Some(1024));
        let request = create_request(
            &model_config,
            "system",
            &[],
            &[],
            &ImageFormat::OpenAi,
            false,
        )?;
        let obj = request.as_object().unwrap();
        let expected = json!({
            "model": "o1",
            "messages": [
                {
                    "role": "developer",
                    "content": "system"
                }
            ],
            "max_completion_tokens": 1024
        });

        for (key, value) in expected.as_object().unwrap() {
            assert_eq!(obj.get(key).unwrap(), value);
        }
        assert!(
            obj.get("reasoning_effort").is_none(),
            "reasoning_effort should be omitted when no explicit suffix is provided"
        );

        Ok(())
    }

    #[test]
    fn test_create_request_o1_medium_effort() -> anyhow::Result<()> {
        let model_config = test_model_config("o1")
            .with_max_tokens(Some(1024))
            .with_thinking_effort(ThinkingEffort::Medium);
        let request = create_request(
            &model_config,
            "system",
            &[],
            &[],
            &ImageFormat::OpenAi,
            false,
        )?;
        let obj = request.as_object().unwrap();

        assert_eq!(obj.get("reasoning_effort"), Some(&json!("medium")));
        assert!(obj.get("thinking_effort").is_none());

        Ok(())
    }

    #[test]
    fn test_create_request_gpt56_max_effort_uses_xhigh() -> anyhow::Result<()> {
        let model_config = test_model_config("gpt-5.6-luna")
            .with_max_tokens(Some(1024))
            .with_thinking_effort(ThinkingEffort::Max);
        let request = create_request(
            &model_config,
            "system",
            &[],
            &[],
            &ImageFormat::OpenAi,
            false,
        )?;
        let obj = request.as_object().unwrap();

        assert_eq!(obj.get("reasoning_effort"), Some(&json!("xhigh")));
        assert!(obj.get("thinking_effort").is_none());

        Ok(())
    }

    #[test]
    fn test_openai_reasoning_effort_gpt6_does_not_support_none() {
        for model in [
            "gpt-6-astra",
            "data_workflow_tools.goose.goose-gpt-6-astra",
            "openrouter/openai/gpt-6-astra",
        ] {
            assert_eq!(
                openai_reasoning_effort_for_thinking(model, ThinkingEffort::Off),
                Some("low".to_string()),
                "{model} Off should map to low, not none"
            );
            assert_eq!(
                openai_reasoning_effort_for_thinking(model, ThinkingEffort::Medium),
                Some("medium".to_string()),
                "{model} medium should remain medium"
            );
            assert!(
                !openai_reasoning_efforts_for_model(model).contains(&"none"),
                "{model} must not advertise none"
            );
        }

        assert_eq!(
            openai_reasoning_effort_for_thinking("gpt-5.6-luna", ThinkingEffort::Off),
            Some("none".to_string())
        );
        assert_eq!(
            openai_reasoning_effort_for_thinking("gpt-5.4", ThinkingEffort::Off),
            Some("none".to_string())
        );
        assert_eq!(
            openai_reasoning_effort_for_thinking("gpt-5", ThinkingEffort::Off),
            Some("low".to_string())
        );
    }

    #[test]
    fn test_create_request_gpt5_pro_max_effort_uses_supported_level() -> anyhow::Result<()> {
        let model_config = test_model_config("gpt-5.2-pro-2025-12-11")
            .with_max_tokens(Some(1024))
            .with_thinking_effort(ThinkingEffort::Max);
        let request = create_request(
            &model_config,
            "system",
            &[],
            &[],
            &ImageFormat::OpenAi,
            false,
        )?;
        let obj = request.as_object().unwrap();

        assert_eq!(obj.get("reasoning_effort"), Some(&json!("high")));
        assert!(obj.get("thinking_effort").is_none());

        Ok(())
    }

    #[test]
    fn test_create_request_o3_custom_reasoning_effort() -> anyhow::Result<()> {
        let model_config = test_model_config("o3-mini")
            .with_max_tokens(Some(1024))
            .with_thinking_effort(ThinkingEffort::High);
        let request = create_request(
            &model_config,
            "system",
            &[],
            &[],
            &ImageFormat::OpenAi,
            false,
        )?;
        let obj = request.as_object().unwrap();
        let expected = json!({
            "model": "o3-mini",
            "messages": [
                {
                    "role": "developer",
                    "content": "system"
                }
            ],
            "reasoning_effort": "high",
            "max_completion_tokens": 1024
        });

        for (key, value) in expected.as_object().unwrap() {
            assert_eq!(obj.get(key).unwrap(), value);
        }
        assert!(obj.get("thinking_effort").is_none());

        Ok(())
    }

    struct StreamingUsageTestResult {
        usage_count: usize,
        usage: Option<ProviderUsage>,
        tool_calls: Vec<String>,
        has_text_content: bool,
        text: String,
        output_token_limit_message_ids: Vec<Option<String>>,
    }

    async fn run_streaming_test(response_lines: &str) -> anyhow::Result<StreamingUsageTestResult> {
        let lines: Vec<String> = response_lines.lines().map(|s| s.to_string()).collect();
        let response_stream = tokio_stream::iter(lines.into_iter().map(Ok));
        let messages = response_to_streaming_message(response_stream);
        pin!(messages);

        let mut result = StreamingUsageTestResult {
            usage_count: 0,
            usage: None,
            tool_calls: Vec::new(),
            has_text_content: false,
            text: String::new(),
            output_token_limit_message_ids: Vec::new(),
        };

        while let Some(Ok((message, usage))) = messages.next().await {
            if let Some(u) = usage {
                result.usage_count += 1;
                result.usage = Some(u);
            }
            if let Some(msg) = message {
                if msg.metadata.output_token_limit_reached {
                    result.output_token_limit_message_ids.push(msg.id.clone());
                }
                for content in &msg.content {
                    match content {
                        MessageContentBlock::ToolRequest(req) => {
                            if let Ok(tool_call) = &req.tool_call {
                                result.tool_calls.push(tool_call.name.to_string());
                            }
                        }
                        MessageContentBlock::Text(text) if !text.text.is_empty() => {
                            result.has_text_content = true;
                            result.text.push_str(&text.text);
                        }
                        _ => {}
                    }
                }
            }
        }

        Ok(result)
    }

    fn assert_usage_yielded_once(
        result: &StreamingUsageTestResult,
        expected_input: i32,
        expected_output: i32,
        expected_total: i32,
    ) {
        assert_eq!(
            result.usage_count, 1,
            "Usage should be yielded exactly once, but was yielded {} times",
            result.usage_count
        );

        let usage = result.usage.as_ref().expect("Expected usage to be present");
        assert_eq!(usage.usage.input_tokens, Some(expected_input));
        assert_eq!(usage.usage.output_tokens, Some(expected_output));
        assert_eq!(usage.usage.total_tokens, Some(expected_total));
    }

    async fn collect_streamed_tool_requests(
        response_lines: &str,
    ) -> Vec<crate::conversation::message::ToolRequest> {
        let lines: Vec<String> = response_lines.lines().map(|s| s.to_string()).collect();
        let response_stream = tokio_stream::iter(lines.into_iter().map(Ok));
        let messages = response_to_streaming_message(response_stream);
        pin!(messages);

        let mut requests = Vec::new();
        while let Some(Ok((message, _usage))) = messages.next().await {
            if let Some(msg) = message {
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
    async fn test_streaming_reassembles_interleaved_parallel_tool_calls() {
        let response_lines = concat!(
            r#"data: {"id":"chatcmpl-par","model":"test-model","choices":[{"index":0,"delta":{"role":"assistant","tool_calls":[{"index":0,"id":"call_a","type":"function","function":{"name":"search","arguments":""}},{"index":1,"id":"call_b","type":"function","function":{"name":"write","arguments":""}}]},"finish_reason":null}]}"#,
            "\n",
            r#"data: {"id":"chatcmpl-par","model":"test-model","choices":[{"index":0,"delta":{"tool_calls":[{"index":1,"function":{"arguments":"{\"path\":"}}]},"finish_reason":null}]}"#,
            "\n",
            r#"data: {"id":"chatcmpl-par","model":"test-model","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"query\":"}}]},"finish_reason":null}]}"#,
            "\n",
            r#"data: {"id":"chatcmpl-par","model":"test-model","choices":[{"index":0,"delta":{"tool_calls":[{"index":1,"function":{"arguments":"\"/tmp/a.md\"}"}}]},"finish_reason":null}]}"#,
            "\n",
            r#"data: {"id":"chatcmpl-par","model":"test-model","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"rust\"}"}}]},"finish_reason":"tool_calls"}]}"#,
            "\n",
            "data: [DONE]",
        );

        let requests = collect_streamed_tool_requests(response_lines).await;

        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests
                .iter()
                .map(|r| r.provider_index())
                .collect::<Vec<_>>(),
            vec![Some(0), Some(1)]
        );

        let search = requests[0].tool_call.as_ref().expect("search args parsed");
        assert_eq!(search.name, "search");
        assert_eq!(search.arguments.as_ref().unwrap()["query"], json!("rust"));

        let write = requests[1].tool_call.as_ref().expect("write args parsed");
        assert_eq!(write.name, "write");
        assert_eq!(
            write.arguments.as_ref().unwrap()["path"],
            json!("/tmp/a.md")
        );
    }

    #[tokio::test]
    async fn test_streaming_marks_length_on_empty_terminal_chunk() -> anyhow::Result<()> {
        let response_lines = r#"
data: {"id":"chatcmpl-limit","model":"test-model","choices":[{"index":0,"delta":{"role":"assistant","content":"Partial answer"},"finish_reason":null}]}
data: {"id":"chatcmpl-limit","model":"test-model","choices":[{"index":0,"delta":{},"finish_reason":"length"}],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}
data: [DONE]
"#;

        let result = run_streaming_test(response_lines).await?;

        assert_eq!(result.text, "Partial answer");
        assert_eq!(
            result.output_token_limit_message_ids,
            vec![Some("chatcmpl-limit".to_string())]
        );
        assert_usage_yielded_once(&result, 10, 5, 15);

        Ok(())
    }

    #[test]
    fn test_get_usage_preserves_provider_totals_with_cache_fields() {
        let usage = get_usage(&json!({
            "prompt_tokens": 120,
            "completion_tokens": 30,
            "total_tokens": 150,
            "cache_read_input_tokens": 80,
            "cache_creation_input_tokens": 20
        }));

        assert_eq!(usage.input_tokens, Some(120));
        assert_eq!(usage.output_tokens, Some(30));
        assert_eq!(usage.total_tokens, Some(150));
        assert_eq!(usage.cache_read_input_tokens, Some(80));
        assert_eq!(usage.cache_write_input_tokens, Some(20));
    }

    #[test]
    fn test_get_usage_reads_openai_prompt_tokens_details() {
        let usage = get_usage(&json!({
            "prompt_tokens": 120,
            "completion_tokens": 30,
            "total_tokens": 150,
            "prompt_tokens_details": {
                "cached_tokens": 80
            }
        }));

        assert_eq!(usage.input_tokens, Some(120));
        assert_eq!(usage.output_tokens, Some(30));
        assert_eq!(usage.total_tokens, Some(150));
        assert_eq!(usage.cache_read_input_tokens, Some(80));
        assert_eq!(usage.cache_write_input_tokens, None);
    }

    #[test]
    fn test_get_usage_reads_nested_usage_with_cache_fields() {
        let usage = get_usage(&json!({
            "id": "chatcmpl_test",
            "object": "chat.completion",
            "usage": {
                "prompt_tokens": 84,
                "completion_tokens": 21,
                "total_tokens": 105,
                "prompt_tokens_details": {
                    "cached_tokens": 60
                },
                "cache_creation_input_tokens": 10
            }
        }));

        assert_eq!(usage.input_tokens, Some(84));
        assert_eq!(usage.output_tokens, Some(21));
        assert_eq!(usage.total_tokens, Some(105));
        assert_eq!(usage.cache_read_input_tokens, Some(60));
        assert_eq!(usage.cache_write_input_tokens, Some(10));
    }

    #[tokio::test]
    async fn test_streamed_multi_tool_response_to_messages() -> anyhow::Result<()> {
        let response_lines = r#"
data: {"model":"us.anthropic.claude-sonnet-4-20250514-v1:0","choices":[{"delta":{"role":"assistant","content":"I'll run both"},"index":0,"finish_reason":null}],"usage":{"prompt_tokens":4982,"completion_tokens":null,"total_tokens":null},"object":"chat.completion.chunk","id":"msg_bdrk_014pifLTHsNZz6Lmtw1ywgDJ","created":1753288340}
data: {"model":"us.anthropic.claude-sonnet-4-20250514-v1:0","choices":[{"delta":{"role":"assistant","content":" `ls` commands in a"},"index":0,"finish_reason":null}],"usage":{"prompt_tokens":4982,"completion_tokens":null,"total_tokens":null},"object":"chat.completion.chunk","id":"msg_bdrk_014pifLTHsNZz6Lmtw1ywgDJ","created":1753288340}
data: {"model":"us.anthropic.claude-sonnet-4-20250514-v1:0","choices":[{"delta":{"role":"assistant","content":" single turn for you -"},"index":0,"finish_reason":null}],"usage":{"prompt_tokens":4982,"completion_tokens":null,"total_tokens":null},"object":"chat.completion.chunk","id":"msg_bdrk_014pifLTHsNZz6Lmtw1ywgDJ","created":1753288340}
data: {"model":"us.anthropic.claude-sonnet-4-20250514-v1:0","choices":[{"delta":{"role":"assistant","content":" one on the current directory an"},"index":0,"finish_reason":null}],"usage":{"prompt_tokens":4982,"completion_tokens":null,"total_tokens":null},"object":"chat.completion.chunk","id":"msg_bdrk_014pifLTHsNZz6Lmtw1ywgDJ","created":1753288340}
data: {"model":"us.anthropic.claude-sonnet-4-20250514-v1:0","choices":[{"delta":{"role":"assistant","content":"d one on the `working_dir`."},"index":0,"finish_reason":null}],"usage":{"prompt_tokens":4982,"completion_tokens":null,"total_tokens":null},"object":"chat.completion.chunk","id":"msg_bdrk_014pifLTHsNZz6Lmtw1ywgDJ","created":1753288340}
data: {"model":"us.anthropic.claude-sonnet-4-20250514-v1:0","choices":[{"delta":{"role":"assistant","content":null,"tool_calls":[{"index":1,"id":"toolu_bdrk_01RMTd7R9DzQjEEWgDwzcBsU","type":"function","function":{"name":"developer__shell","arguments":""}}]},"index":0,"finish_reason":null}],"usage":{"prompt_tokens":4982,"completion_tokens":null,"total_tokens":null},"object":"chat.completion.chunk","id":"msg_bdrk_014pifLTHsNZz6Lmtw1ywgDJ","created":1753288341}
data: {"model":"us.anthropic.claude-sonnet-4-20250514-v1:0","choices":[{"delta":{"role":"assistant","content":null,"tool_calls":[{"index":1,"function":{"arguments":""}}]},"index":0,"finish_reason":null}],"usage":{"prompt_tokens":4982,"completion_tokens":null,"total_tokens":null},"object":"chat.completion.chunk","id":"msg_bdrk_014pifLTHsNZz6Lmtw1ywgDJ","created":1753288341}
data: {"model":"us.anthropic.claude-sonnet-4-20250514-v1:0","choices":[{"delta":{"role":"assistant","content":null,"tool_calls":[{"index":1,"function":{"arguments":"{\""}}]},"index":0,"finish_reason":null}],"usage":{"prompt_tokens":4982,"completion_tokens":null,"total_tokens":null},"object":"chat.completion.chunk","id":"msg_bdrk_014pifLTHsNZz6Lmtw1ywgDJ","created":1753288341}
data: {"model":"us.anthropic.claude-sonnet-4-20250514-v1:0","choices":[{"delta":{"role":"assistant","content":null,"tool_calls":[{"index":1,"function":{"arguments":"command\": \"l"}}]},"index":0,"finish_reason":null}],"usage":{"prompt_tokens":4982,"completion_tokens":null,"total_tokens":null},"object":"chat.completion.chunk","id":"msg_bdrk_014pifLTHsNZz6Lmtw1ywgDJ","created":1753288341}
data: {"model":"us.anthropic.claude-sonnet-4-20250514-v1:0","choices":[{"delta":{"role":"assistant","content":null,"tool_calls":[{"index":1,"function":{"arguments":"s\"}"}}]},"index":0,"finish_reason":null}],"usage":{"prompt_tokens":4982,"completion_tokens":null,"total_tokens":null},"object":"chat.completion.chunk","id":"msg_bdrk_014pifLTHsNZz6Lmtw1ywgDJ","created":1753288341}
data: {"model":"us.anthropic.claude-sonnet-4-20250514-v1:0","choices":[{"delta":{"role":"assistant","content":null,"tool_calls":[{"index":2,"id":"toolu_bdrk_016bgVTGZdpjP8ehjMWp9cWW","type":"function","function":{"name":"developer__shell","arguments":""}}]},"index":0,"finish_reason":null}],"usage":{"prompt_tokens":4982,"completion_tokens":null,"total_tokens":null},"object":"chat.completion.chunk","id":"msg_bdrk_014pifLTHsNZz6Lmtw1ywgDJ","created":1753288341}
data: {"model":"us.anthropic.claude-sonnet-4-20250514-v1:0","choices":[{"delta":{"role":"assistant","content":null,"tool_calls":[{"index":2,"function":{"arguments":""}}]},"index":0,"finish_reason":null}],"usage":{"prompt_tokens":4982,"completion_tokens":null,"total_tokens":null},"object":"chat.completion.chunk","id":"msg_bdrk_014pifLTHsNZz6Lmtw1ywgDJ","created":1753288341}
data: {"model":"us.anthropic.claude-sonnet-4-20250514-v1:0","choices":[{"delta":{"role":"assistant","content":null,"tool_calls":[{"index":2,"function":{"arguments":"{\""}}]},"index":0,"finish_reason":null}],"usage":{"prompt_tokens":4982,"completion_tokens":null,"total_tokens":null},"object":"chat.completion.chunk","id":"msg_bdrk_014pifLTHsNZz6Lmtw1ywgDJ","created":1753288342}
data: {"model":"us.anthropic.claude-sonnet-4-20250514-v1:0","choices":[{"delta":{"role":"assistant","content":null,"tool_calls":[{"index":2,"function":{"arguments":"command\""}}]},"index":0,"finish_reason":null}],"usage":{"prompt_tokens":4982,"completion_tokens":null,"total_tokens":null},"object":"chat.completion.chunk","id":"msg_bdrk_014pifLTHsNZz6Lmtw1ywgDJ","created":1753288342}
data: {"model":"us.anthropic.claude-sonnet-4-20250514-v1:0","choices":[{"delta":{"role":"assistant","content":null,"tool_calls":[{"index":2,"function":{"arguments":": \"ls wor"}}]},"index":0,"finish_reason":null}],"usage":{"prompt_tokens":4982,"completion_tokens":null,"total_tokens":null},"object":"chat.completion.chunk","id":"msg_bdrk_014pifLTHsNZz6Lmtw1ywgDJ","created":1753288342}
data: {"model":"us.anthropic.claude-sonnet-4-20250514-v1:0","choices":[{"delta":{"role":"assistant","content":null,"tool_calls":[{"index":2,"function":{"arguments":"king_dir"}}]},"index":0,"finish_reason":null}],"usage":{"prompt_tokens":4982,"completion_tokens":null,"total_tokens":null},"object":"chat.completion.chunk","id":"msg_bdrk_014pifLTHsNZz6Lmtw1ywgDJ","created":1753288342}
data: {"model":"us.anthropic.claude-sonnet-4-20250514-v1:0","choices":[{"delta":{"role":"assistant","content":null,"tool_calls":[{"index":2,"function":{"arguments":"\"}"}}]},"index":0,"finish_reason":null}],"usage":{"prompt_tokens":4982,"completion_tokens":null,"total_tokens":null},"object":"chat.completion.chunk","id":"msg_bdrk_014pifLTHsNZz6Lmtw1ywgDJ","created":1753288342}
data: {"model":"us.anthropic.claude-sonnet-4-20250514-v1:0","choices":[{"delta":{"role":"assistant","content":""},"index":0,"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":4982,"completion_tokens":122,"total_tokens":5104},"object":"chat.completion.chunk","id":"msg_bdrk_014pifLTHsNZz6Lmtw1ywgDJ","created":1753288342}
data: [DONE]
"#;

        let result = run_streaming_test(response_lines).await?;
        assert_eq!(
            result.tool_calls.len(),
            2,
            "Expected 2 tool calls, got {}",
            result.tool_calls.len()
        );
        assert!(result
            .tool_calls
            .iter()
            .all(|name| name == "developer__shell"));

        assert_usage_yielded_once(&result, 4982, 122, 5104);

        Ok(())
    }

    #[tokio::test]
    async fn test_streaming_empty_finish_reason_is_not_terminal() -> anyhow::Result<()> {
        let response_lines = r#"
data: {"model":"m","choices":[{"delta":{"role":"assistant","content":"Checking."},"index":0,"finish_reason":""}],"object":"chat.completion.chunk","id":"1","created":1753288340}
data: {"model":"m","choices":[{"delta":{"role":"assistant","content":null,"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"developer__shell","arguments":""}}]},"index":0,"finish_reason":""}],"object":"chat.completion.chunk","id":"1","created":1753288340}
data: {"model":"m","choices":[{"delta":{"role":"assistant","content":null,"tool_calls":[{"index":0,"function":{"arguments":"{\"command\""}}]},"index":0,"finish_reason":""}],"object":"chat.completion.chunk","id":"1","created":1753288340}
data: {"model":"m","choices":[{"delta":{"role":"assistant","content":null,"tool_calls":[{"index":0,"function":{"arguments":": \"ls\"}"}}]},"index":0,"finish_reason":""}],"object":"chat.completion.chunk","id":"1","created":1753288340}
data: {"model":"m","choices":[{"delta":{"role":"assistant","content":""},"index":0,"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":100,"completion_tokens":20,"total_tokens":120},"object":"chat.completion.chunk","id":"1","created":1753288340}
data: [DONE]
"#;

        let result = run_streaming_test(response_lines).await?;

        assert!(result.has_text_content, "Expected text content in response");
        assert_eq!(
            result.tool_calls,
            vec!["developer__shell"],
            "tool call must survive intermediate empty-string finish_reason"
        );
        assert_usage_yielded_once(&result, 100, 20, 120);

        Ok(())
    }

    #[tokio::test]
    async fn test_streaming_metadata_without_usage() -> anyhow::Result<()> {
        let response_lines = r#"
data: {"id":"chatcmpl-no-usage","model":"test-model","choices":[{"index":0,"delta":{"role":"assistant","content":"Hello"},"finish_reason":null}]}
data: {"id":"chatcmpl-no-usage","model":"test-model","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}
data: [DONE]
"#;

        let result = run_streaming_test(response_lines).await?;

        assert_eq!(result.usage_count, 1);
        let usage = result.usage.unwrap();
        assert_eq!(usage.model, "test-model");
        assert_eq!(usage.usage, Usage::default());
        assert_eq!(usage.finish_reasons, Some(vec!["stop".to_string()]));
        assert_eq!(usage.response_id.as_deref(), Some("chatcmpl-no-usage"));

        Ok(())
    }

    #[tokio::test]
    async fn test_openrouter_streaming_usage_yielded_once() -> anyhow::Result<()> {
        let response_lines = r#"
data: {"id":"gen-1768896871-9HgAQqS1Z72C6gApaidi","provider":"OpenInference","model":"openai/gpt-oss-120b:free","object":"chat.completion.chunk","created":1768896871,"choices":[{"index":0,"delta":{"role":"assistant","content":"","reasoning":null,"reasoning_details":[]},"finish_reason":null,"native_finish_reason":null,"logprobs":null}]}
data: {"id":"gen-1768896871-9HgAQqS1Z72C6gApaidi","provider":"OpenInference","model":"openai/gpt-oss-120b:free","object":"chat.completion.chunk","created":1768896871,"choices":[{"index":0,"delta":{"role":"assistant","content":"There","reasoning":"","reasoning_details":[]},"finish_reason":null,"native_finish_reason":null,"logprobs":null}]}
data: {"id":"gen-1768896871-9HgAQqS1Z72C6gApaidi","provider":"OpenInference","model":"openai/gpt-oss-120b:free","object":"chat.completion.chunk","created":1768896871,"choices":[{"index":0,"delta":{"role":"assistant","content":" are","reasoning":null,"reasoning_details":[]},"finish_reason":null,"native_finish_reason":null,"logprobs":null}]}
data: {"id":"gen-1768896871-9HgAQqS1Z72C6gApaidi","provider":"OpenInference","model":"openai/gpt-oss-120b:free","object":"chat.completion.chunk","created":1768896871,"choices":[{"index":0,"delta":{"role":"assistant","content":" **47**","reasoning":null,"reasoning_details":[]},"finish_reason":null,"native_finish_reason":null,"logprobs":null}]}
data: {"id":"gen-1768896871-9HgAQqS1Z72C6gApaidi","provider":"OpenInference","model":"openai/gpt-oss-120b:free","object":"chat.completion.chunk","created":1768896871,"choices":[{"index":0,"delta":{"role":"assistant","content":" files.","reasoning":null,"reasoning_details":[]},"finish_reason":null,"native_finish_reason":null,"logprobs":null}]}
data: {"id":"gen-1768896871-9HgAQqS1Z72C6gApaidi","provider":"OpenInference","model":"openai/gpt-oss-120b:free","object":"chat.completion.chunk","created":1768896871,"choices":[{"index":0,"delta":{"role":"assistant","content":"","reasoning":null,"reasoning_details":[]},"finish_reason":"stop","native_finish_reason":"stop","logprobs":null}]}
data: {"id":"gen-1768896871-9HgAQqS1Z72C6gApaidi","provider":"OpenInference","model":"openai/gpt-oss-120b:free","object":"chat.completion.chunk","created":1768896871,"choices":[{"index":0,"delta":{"role":"assistant","content":""},"finish_reason":null,"native_finish_reason":null,"logprobs":null}],"usage":{"prompt_tokens":7007,"completion_tokens":49,"total_tokens":7056}}
data: [DONE]
"#;

        let result = run_streaming_test(response_lines).await?;

        assert!(result.has_text_content, "Expected text content in response");
        assert_usage_yielded_once(&result, 7007, 49, 7056);
        let usage = result.usage.as_ref().unwrap();
        assert_eq!(
            usage.finish_reasons.as_deref(),
            Some(&["stop".to_string()][..])
        );
        assert_eq!(
            usage.response_id.as_deref(),
            Some("gen-1768896871-9HgAQqS1Z72C6gApaidi")
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_openai_gpt5_streaming_usage_yielded_once() -> anyhow::Result<()> {
        let response_lines = r#"
data: {"id":"chatcmpl-Bk9Ye6Y0t9E7bC3DOMxCpW8eJkTKU","object":"chat.completion.chunk","created":1737368310,"model":"gpt-5.2-1106-preview","service_tier":"default","system_fingerprint":"fp_5f325d54e6","choices":[{"index":0,"delta":{"role":"assistant","content":null,"tool_calls":[{"index":0,"id":"call_x4CIvBVfQhYMhyO0T1VEddua","type":"function","function":{"name":"developer__shell","arguments":""}}],"refusal":null},"logprobs":null,"finish_reason":null}],"usage":null}
data: {"id":"chatcmpl-Bk9Ye6Y0t9E7bC3DOMxCpW8eJkTKU","object":"chat.completion.chunk","created":1737368310,"model":"gpt-5.2-1106-preview","service_tier":"default","system_fingerprint":"fp_5f325d54e6","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"command\":\"ls ~/Desktop | wc -l\"}"}}]},"logprobs":null,"finish_reason":null}],"usage":null}
data: {"id":"chatcmpl-Bk9Ye6Y0t9E7bC3DOMxCpW8eJkTKU","object":"chat.completion.chunk","created":1737368310,"model":"gpt-5.2-1106-preview","service_tier":"default","system_fingerprint":"fp_5f325d54e6","choices":[{"index":0,"delta":{},"logprobs":null,"finish_reason":"tool_calls"}],"usage":null}
data: {"id":"chatcmpl-Bk9Ye6Y0t9E7bC3DOMxCpW8eJkTKU","object":"chat.completion.chunk","created":1737368310,"model":"gpt-5.2-1106-preview","service_tier":"default","system_fingerprint":"fp_5f325d54e6","choices":[],"usage":{"prompt_tokens":8320,"completion_tokens":172,"total_tokens":8492}}
data: [DONE]
"#;

        let result = run_streaming_test(response_lines).await?;

        assert_eq!(result.tool_calls.len(), 1, "Expected 1 tool call");
        assert_eq!(result.tool_calls[0], "developer__shell");
        assert_usage_yielded_once(&result, 8320, 172, 8492);
        assert_eq!(
            result.usage.as_ref().map(|usage| usage.model.as_str()),
            Some("gpt-5.2-1106-preview")
        );
        let usage = result.usage.as_ref().unwrap();
        assert_eq!(
            usage.finish_reasons.as_deref(),
            Some(&["tool_calls".to_string()][..])
        );
        assert_eq!(
            usage.response_id.as_deref(),
            Some("chatcmpl-Bk9Ye6Y0t9E7bC3DOMxCpW8eJkTKU")
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_tetrate_claude_streaming_usage_yielded_once() -> anyhow::Result<()> {
        let response_lines = r#"
data: {"id":"msg_01BbvMfNhbdm2hmmTbWjaeYt","choices":[{"index":0,"delta":{"role":"assistant"}}],"created":1768898776,"model":"claude-sonnet-4-5-20250929","object":"chat.completion.chunk"}
data: {"id":"msg_01BbvMfNhbdm2hmmTbWjaeYt","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"toolu_011Yj5pGczhs1597iLXp5XJK","type":"function","function":{"name":"developer__shell","arguments":""}}]}}],"created":1768898776,"model":"claude-sonnet-4-5-20250929","object":"chat.completion.chunk"}
data: {"id":"msg_01BbvMfNhbdm2hmmTbWjaeYt","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"type":"function","function":{"arguments":"{\"command\": \"find ~/Desktop -type f | wc -l\"}"}}]}}],"created":1768898776,"model":"claude-sonnet-4-5-20250929","object":"chat.completion.chunk"}
data: {"id":"msg_01BbvMfNhbdm2hmmTbWjaeYt","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}],"created":1768898776,"model":"claude-sonnet-4-5-20250929","object":"chat.completion.chunk","usage":{"completion_tokens":79,"prompt_tokens":12376,"total_tokens":12455}}
data: [DONE]
"#;

        let result = run_streaming_test(response_lines).await?;

        assert_eq!(result.tool_calls.len(), 1, "Expected 1 tool call");
        assert_eq!(result.tool_calls[0], "developer__shell");
        assert_usage_yielded_once(&result, 12376, 79, 12455);

        Ok(())
    }

    #[tokio::test]
    async fn test_azure_annotation_chunk_without_delta_does_not_fail() -> anyhow::Result<()> {
        let response_lines = r#"
data: {"id":"chatcmpl-test","object":"chat.completion.chunk","created":1234567890,"model":"gpt-5.4","choices":[{"index":0,"delta":{"role":"assistant","content":"Hello"},"finish_reason":null}],"usage":null}
data: {"choices":[{"content_filter_offsets":{"check_offset":5,"start_offset":5,"end_offset":5},"content_filter_results":{"hate":{"filtered":false,"severity":"safe"},"self_harm":{"filtered":false,"severity":"safe"},"sexual":{"filtered":false,"severity":"safe"},"violence":{"filtered":false,"severity":"safe"}},"finish_reason":null,"index":0}],"created":0,"id":"","model":"","object":""}
data: {"id":"chatcmpl-test","object":"chat.completion.chunk","created":1234567891,"model":"gpt-5.4","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":1,"total_tokens":11}}
data: [DONE]
"#;

        let result = run_streaming_test(response_lines).await?;

        assert!(result.has_text_content, "Expected text content in response");
        assert_usage_yielded_once(&result, 10, 1, 11);

        Ok(())
    }

    #[test]
    fn test_response_to_message_with_nested_extra_content() -> anyhow::Result<()> {
        let response = json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "tool_calls": [{
                        "id": "call_456",
                        "type": "function",
                        "function": {
                            "name": "test_tool",
                            "arguments": "{}"
                        },
                        "extra_content": {
                            "google": {
                                "thought_signature": "nested_sig_xyz789"
                            }
                        }
                    }]
                }
            }]
        });

        let message = response_to_message(&response)?;
        assert_eq!(message.content.len(), 1);

        if let MessageContentBlock::ToolRequest(request) = &message.content[0] {
            assert!(request.tool_call.is_ok());
            assert!(request.metadata.is_some());
            let metadata = request.metadata.as_ref().unwrap();
            let extra_content = metadata.get("extra_content").unwrap();
            assert_eq!(
                extra_content["google"]["thought_signature"],
                "nested_sig_xyz789"
            );
        } else {
            panic!("Expected ToolRequest content");
        }

        Ok(())
    }

    #[test]
    fn test_response_to_message_with_multiple_extra_fields() -> anyhow::Result<()> {
        let response = json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "tool_calls": [{
                        "id": "call_789",
                        "type": "function",
                        "function": {
                            "name": "test_tool",
                            "arguments": "{}"
                        },
                        "thoughtSignature": "sig_top_level",
                        "extra_content": {
                            "google": {
                                "thought_signature": "sig_nested"
                            }
                        },
                        "custom_field": "custom_value"
                    }]
                }
            }]
        });

        let message = response_to_message(&response)?;

        if let MessageContentBlock::ToolRequest(request) = &message.content[0] {
            let metadata = request.metadata.as_ref().unwrap();
            assert_eq!(metadata.get("thoughtSignature").unwrap(), "sig_top_level");
            assert_eq!(
                metadata.get("extra_content").unwrap()["google"]["thought_signature"],
                "sig_nested"
            );
            assert_eq!(metadata.get("custom_field").unwrap(), "custom_value");
        } else {
            panic!("Expected ToolRequest content");
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_streaming_response_with_nested_extra_content() -> anyhow::Result<()> {
        let response_lines = r#"data: {"model":"test-model","choices":[{"delta":{"role":"assistant","tool_calls":[{"extra_content":{"google":{"thought_signature":"nested_stream_sig"}},"id":"call_nested","function":{"name":"test_tool","arguments":"{}"},"type":"function","index":0}]},"index":0,"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":100,"completion_tokens":10,"total_tokens":110},"object":"chat.completion.chunk","id":"test-id","created":1234567890}
data: [DONE]"#;

        let response_stream =
            tokio_stream::iter(response_lines.lines().map(|line| Ok(line.to_string())));
        let messages = response_to_streaming_message(response_stream);
        pin!(messages);

        while let Some(Ok((message, _usage))) = messages.next().await {
            if let Some(msg) = message {
                if let MessageContentBlock::ToolRequest(request) = &msg.content[0] {
                    assert!(request.tool_call.is_ok());
                    assert!(request.metadata.is_some());
                    let metadata = request.metadata.as_ref().unwrap();
                    let extra_content = metadata.get("extra_content").unwrap();
                    assert_eq!(
                        extra_content["google"]["thought_signature"],
                        "nested_stream_sig"
                    );
                    return Ok(());
                }
            }
        }

        panic!("Expected tool call message with nested extra_content metadata");
    }

    #[tokio::test]
    async fn test_streaming_non_object_arguments_does_not_panic() -> anyhow::Result<()> {
        // Streamed tool call whose arguments are valid JSON but NOT an object.
        // Must yield an INVALID_PARAMS tool error, not panic via rmcp `object()`.
        let response_lines = r#"data: {"model":"test-model","choices":[{"delta":{"role":"assistant","tool_calls":[{"id":"call_bad","function":{"name":"test_tool","arguments":"[1, "},"type":"function","index":0}]},"index":0,"finish_reason":null}],"object":"chat.completion.chunk","id":"test-id","created":1234567890}
data: {"model":"test-model","choices":[{"delta":{"tool_calls":[{"function":{"arguments":"2, 3]"},"index":0}]},"index":0,"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":100,"completion_tokens":10,"total_tokens":110},"object":"chat.completion.chunk","id":"test-id","created":1234567890}
data: [DONE]"#;

        let response_stream =
            tokio_stream::iter(response_lines.lines().map(|line| Ok(line.to_string())));
        let messages = response_to_streaming_message(response_stream);
        pin!(messages);

        let mut found_tool_error = false;
        let mut usage_count = 0;
        while let Some(result) = messages.next().await {
            let (message, usage) = result?;
            if usage.is_some() {
                usage_count += 1;
            }
            if let Some(msg) = message {
                if let MessageContentBlock::ToolRequest(request) = &msg.content[0] {
                    assert!(!msg.metadata.output_token_limit_reached);
                    assert_eq!(msg.id.as_deref(), Some("test-id"));
                    match &request.tool_call {
                        Err(ErrorData {
                            code: ErrorCode::INVALID_PARAMS,
                            message: m,
                            ..
                        }) => {
                            assert!(m.contains("must be a JSON object"));
                            assert!(
                                m.contains("test_tool"),
                                "error must name the original tool so the model can retry it: {m}"
                            );
                            found_tool_error = true;
                        }
                        _ => panic!("expected INVALID_PARAMS for non-object streamed args"),
                    }
                }
            }
        }

        assert!(found_tool_error, "expected a tool request message");
        assert_eq!(usage_count, 1);
        Ok(())
    }

    #[tokio::test]
    async fn test_streaming_length_rejects_tool_calls_regardless_of_arguments() -> anyhow::Result<()>
    {
        let response_lines = r#"data: {"model":"test-model","choices":[{"delta":{"role":"assistant","tool_calls":[{"id":"call_empty","function":{"name":"empty_tool","arguments":""},"type":"function","index":0},{"id":"call_valid","function":{"name":"valid_tool","arguments":"{\"value\":"},"type":"function","index":1}]},"index":0,"finish_reason":null}],"object":"chat.completion.chunk","id":"test-id","created":1234567890}
data: {"model":"test-model","choices":[{"delta":{"tool_calls":[{"function":{"arguments":""},"index":0},{"function":{"arguments":"true}"},"index":1}]},"index":0,"finish_reason":"length"}],"usage":{"prompt_tokens":100,"completion_tokens":10,"total_tokens":110},"object":"chat.completion.chunk","id":"test-id","created":1234567890}
data: [DONE]"#;

        let response_stream =
            tokio_stream::iter(response_lines.lines().map(|line| Ok(line.to_string())));
        let messages = response_to_streaming_message(response_stream);
        pin!(messages);

        let mut tool_error_ids = Vec::new();
        let mut usage_count = 0;
        while let Some(result) = messages.next().await {
            let (message, usage) = result?;
            if usage.is_some() {
                usage_count += 1;
            }
            if let Some(msg) = message {
                assert!(msg.metadata.output_token_limit_reached);
                assert_eq!(msg.id.as_deref(), Some("test-id"));
                for content in msg.content {
                    if let MessageContentBlock::ToolRequest(request) = content {
                        let request_id = request.id.clone();
                        match request.tool_call {
                            Err(error) => {
                                assert_eq!(error.code, ErrorCode::INVALID_PARAMS);
                                assert!(error.message.contains("output token limit"));
                                tool_error_ids.push(request_id);
                            }
                            Ok(_) => panic!("length-terminated tool call must not be executable"),
                        }
                    }
                }
            }
        }

        assert_eq!(tool_error_ids, vec!["call_empty", "call_valid"]);
        assert_eq!(usage_count, 1);
        Ok(())
    }

    #[tokio::test]
    async fn test_streaming_response_extracts_inline_think_blocks() -> anyhow::Result<()> {
        let response_lines = concat!(
            "data: {\"id\":\"chunk-1\",\"choices\":[{\"delta\":{\"content\":\"<thi\"},\"index\":0,\"finish_reason\":null}]}\n",
            "data: {\"id\":\"chunk-1\",\"choices\":[{\"delta\":{\"content\":\"nk>x</thi\"},\"index\":0,\"finish_reason\":null}]}\n",
            "data: {\"id\":\"chunk-1\",\"choices\":[{\"delta\":{\"content\":\"nk>y\"},\"index\":0,\"finish_reason\":\"stop\"}]}\n",
            "data: [DONE]\n"
        );

        let response_stream =
            tokio_stream::iter(response_lines.lines().map(|line| Ok(line.to_string())));
        let mut messages = std::pin::pin!(response_to_streaming_message(response_stream));

        let mut text = String::new();
        let mut thinking = String::new();

        while let Some(result) = messages.next().await {
            let (message, _) = result?;
            if let Some(message) = message {
                for item in message.content {
                    match item {
                        MessageContentBlock::Text(text_content) => {
                            text.push_str(&text_content.text)
                        }
                        MessageContentBlock::Thinking(thinking_content) => {
                            thinking.push_str(&thinking_content.thinking)
                        }
                        _ => {}
                    }
                }
            }
        }

        assert_eq!(text, "y");
        assert_eq!(thinking, "x");

        Ok(())
    }

    #[tokio::test]
    async fn test_streaming_truncated_inline_think_preserves_output_token_limit(
    ) -> anyhow::Result<()> {
        let response_lines = concat!(
            "data: {\"id\":\"chunk-1\",\"model\":\"test-model\",\"choices\":[{\"delta\":{\"content\":\"<think>unfinished reasoning\"},\"index\":0,\"finish_reason\":null}]}\n",
            "data: {\"id\":\"chunk-1\",\"choices\":[{\"delta\":{},\"index\":0,\"finish_reason\":\"length\"}],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":5,\"total_tokens\":15}}\n",
            "data: [DONE]\n"
        );

        let response_stream =
            tokio_stream::iter(response_lines.lines().map(|line| Ok(line.to_string())));
        let mut messages = std::pin::pin!(response_to_streaming_message(response_stream));
        let mut streamed_messages = Vec::new();
        let mut usage_count = 0;

        while let Some(result) = messages.next().await {
            let (message, usage) = result?;
            if usage.is_some() {
                usage_count += 1;
            }
            if let Some(message) = message {
                streamed_messages.push(message);
            }
        }

        assert_eq!(usage_count, 1);
        assert_eq!(
            streamed_messages
                .iter()
                .filter(|message| message.metadata.output_token_limit_reached)
                .count(),
            1
        );
        let trailing_message = streamed_messages
            .last()
            .expect("expected trailing thinking");
        assert!(trailing_message.metadata.output_token_limit_reached);
        assert!(matches!(
            trailing_message.content.as_slice(),
            [MessageContentBlock::Thinking(thinking)]
                if thinking.thinking == "unfinished reasoning"
        ));

        Ok(())
    }

    #[tokio::test]
    async fn test_streaming_partial_think_tag_emits_one_output_limit_marker() -> anyhow::Result<()>
    {
        let response_lines = concat!(
            "data: {\"id\":\"chunk-1\",\"model\":\"test-model\",\"choices\":[{\"delta\":{\"content\":\"<thi\"},\"index\":0,\"finish_reason\":null}]}\n",
            "data: {\"id\":\"chunk-1\",\"choices\":[{\"delta\":{},\"index\":0,\"finish_reason\":\"length\"}],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":5,\"total_tokens\":15}}\n",
            "data: [DONE]\n"
        );

        let response_stream =
            tokio_stream::iter(response_lines.lines().map(|line| Ok(line.to_string())));
        let mut messages = std::pin::pin!(response_to_streaming_message(response_stream));
        let mut streamed_messages = Vec::new();
        let mut usage_count = 0;

        while let Some(result) = messages.next().await {
            let (message, usage) = result?;
            usage_count += usize::from(usage.is_some());
            if let Some(message) = message {
                streamed_messages.push(message);
            }
        }

        assert_eq!(usage_count, 1);
        let marked_messages: Vec<_> = streamed_messages
            .iter()
            .filter(|message| message.metadata.output_token_limit_reached)
            .collect();
        assert_eq!(marked_messages.len(), 1);
        assert_eq!(marked_messages[0].id.as_deref(), Some("chunk-1"));
        assert_eq!(marked_messages[0].as_concat_text(), "<thi");

        Ok(())
    }

    #[tokio::test]
    async fn test_streaming_suppresses_inline_think_when_structured_reasoning_follows(
    ) -> anyhow::Result<()> {
        // Inline <think>...</think> arrives in an early content chunk, then
        // reasoning_content arrives in a later chunk. The inline thinking
        // should be discarded in favor of the structured reasoning so users
        // do not get duplicated reasoning output.
        let response_lines = concat!(
            "data: {\"id\":\"chunk-1\",\"choices\":[{\"delta\":{\"content\":\"<think>inline reasoning</think>Hi\"},\"index\":0,\"finish_reason\":null}]}\n",
            "data: {\"id\":\"chunk-2\",\"choices\":[{\"delta\":{\"reasoning_content\":\"structured reasoning\"},\"index\":0,\"finish_reason\":null}]}\n",
            "data: {\"id\":\"chunk-3\",\"choices\":[{\"delta\":{\"content\":\" there\"},\"index\":0,\"finish_reason\":\"stop\"}]}\n",
            "data: [DONE]\n"
        );

        let response_stream =
            tokio_stream::iter(response_lines.lines().map(|line| Ok(line.to_string())));
        let mut messages = std::pin::pin!(response_to_streaming_message(response_stream));

        let mut text = String::new();
        let mut thinking = String::new();

        while let Some(result) = messages.next().await {
            let (message, _) = result?;
            if let Some(message) = message {
                for item in message.content {
                    match item {
                        MessageContentBlock::Text(text_content) => {
                            text.push_str(&text_content.text)
                        }
                        MessageContentBlock::Thinking(thinking_content) => {
                            thinking.push_str(&thinking_content.thinking)
                        }
                        _ => {}
                    }
                }
            }
        }

        assert_eq!(text, "Hi there");
        assert_eq!(thinking, "structured reasoning");

        Ok(())
    }

    #[test]
    fn test_response_to_message_with_reasoning_content() -> anyhow::Result<()> {
        // Test capturing reasoning_content from DeepSeek reasoning models
        let response = json!({
            "choices": [{
                "role": "assistant",
                "message": {
                    "reasoning_content": "Let me think about this step by step...",
                    "content": "The answer is 9.11 is greater than 9.8"
                }
            }],
            "usage": {
                "input_tokens": 10,
                "output_tokens": 25,
                "total_tokens": 35
            }
        });

        let message = response_to_message(&response)?;
        assert_eq!(message.content.len(), 2);

        // First should be thinking content (reasoning is mapped to thinking)
        if let MessageContentBlock::Thinking(thinking) = &message.content[0] {
            assert_eq!(thinking.thinking, "Let me think about this step by step...");
        } else {
            panic!("Expected Thinking content, got {:?}", message.content[0]);
        }

        // Second should be text content
        if let MessageContentBlock::Text(text) = &message.content[1] {
            assert_eq!(text.text, "The answer is 9.11 is greater than 9.8");
        } else {
            panic!("Expected Text content");
        }

        Ok(())
    }

    #[test]
    fn test_response_to_message_extracts_inline_think_blocks() -> anyhow::Result<()> {
        let response = json!({
            "choices": [{
                "role": "assistant",
                "message": {
                    "content": "<think>internal reasoning</think>Visible answer"
                }
            }]
        });

        let message = response_to_message(&response)?;
        assert_eq!(message.content.len(), 2);

        if let MessageContentBlock::Thinking(thinking) = &message.content[0] {
            assert_eq!(thinking.thinking, "internal reasoning");
        } else {
            panic!("Expected Thinking content, got {:?}", message.content[0]);
        }

        if let MessageContentBlock::Text(text) = &message.content[1] {
            assert_eq!(text.text, "Visible answer");
        } else {
            panic!("Expected Text content");
        }

        Ok(())
    }

    #[test]
    fn test_response_to_message_prefers_structured_reasoning_over_inline_think(
    ) -> anyhow::Result<()> {
        let response = json!({
            "choices": [{
                "role": "assistant",
                "message": {
                    "reasoning_content": "structured reasoning",
                    "content": "<think>inline reasoning</think>Visible answer"
                }
            }]
        });

        let message = response_to_message(&response)?;
        assert_eq!(message.content.len(), 2);

        if let MessageContentBlock::Thinking(thinking) = &message.content[0] {
            assert_eq!(thinking.thinking, "structured reasoning");
        } else {
            panic!("Expected Thinking content");
        }

        if let MessageContentBlock::Text(text) = &message.content[1] {
            assert_eq!(text.text, "Visible answer");
        } else {
            panic!("Expected Text content");
        }

        Ok(())
    }

    #[test]
    fn test_format_messages_with_reasoning_content() -> anyhow::Result<()> {
        // Test that reasoning_content is properly included in formatted messages
        let mut message = Message::assistant()
            .with_content(MessageContentBlock::thinking(
                "Thinking through the problem...",
                "",
            ))
            .with_text("The result is 42");

        // Add a tool call to test that reasoning_content works with tool calls
        message = message.with_tool_request(
            "tool1",
            Ok(rmcp::model::CallToolRequestParams::new("test_tool")
                .with_arguments(rmcp::object!({"param": "value"}))),
        );

        let spec = format_messages_with_options(
            &[message],
            &ImageFormat::OpenAi,
            OpenAiFormatOptions {
                preserve_thinking_context: true,
                ..Default::default()
            },
        );

        assert_eq!(spec.len(), 1);
        assert_eq!(spec[0]["role"], "assistant");

        // Should have reasoning_content field
        assert!(spec[0].get("reasoning_content").is_some());
        assert_eq!(
            spec[0]["reasoning_content"],
            "Thinking through the problem..."
        );

        // Should have content
        assert_eq!(spec[0]["content"], "The result is 42");

        // Should have tool_calls
        assert!(spec[0]["tool_calls"].is_array());
        assert_eq!(spec[0]["tool_calls"][0]["function"]["name"], "test_tool");

        Ok(())
    }

    #[test]
    fn test_format_messages_preserves_reasoning_content_for_legacy_compat() -> anyhow::Result<()> {
        let message = Message::assistant()
            .with_content(MessageContentBlock::thinking(
                "Thinking through the problem...",
                "",
            ))
            .with_text("The result is 42");

        let spec = format_messages(&[message], &ImageFormat::OpenAi);

        assert_eq!(spec.len(), 1);
        assert_eq!(spec[0]["content"], "The result is 42");
        assert_eq!(
            spec[0]["reasoning_content"],
            "Thinking through the problem..."
        );

        Ok(())
    }

    #[test]
    fn test_format_messages_with_options_can_omit_reasoning_content() -> anyhow::Result<()> {
        let message = Message::assistant()
            .with_content(MessageContentBlock::thinking(
                "Thinking through the problem...",
                "",
            ))
            .with_text("The result is 42");

        let spec = format_messages_with_options(
            &[message],
            &ImageFormat::OpenAi,
            OpenAiFormatOptions {
                preserve_thinking_context: false,
                ..Default::default()
            },
        );

        assert_eq!(spec.len(), 1);
        assert_eq!(spec[0]["content"], "The result is 42");
        assert!(spec[0].get("reasoning_content").is_none());

        Ok(())
    }

    #[test]
    fn test_create_request_preserves_reasoning_content_for_legacy_compat() -> anyhow::Result<()> {
        let model_config = test_model_config("deepseek-reasoner").with_max_tokens(Some(1024));
        let message = Message::assistant()
            .with_content(MessageContentBlock::thinking("preserve this", ""))
            .with_tool_request(
                "tool1",
                Ok(rmcp::model::CallToolRequestParams::new("test_tool")
                    .with_arguments(rmcp::object!({}))),
            );

        let request = create_request(
            &model_config,
            "system",
            &[message],
            &[],
            &ImageFormat::OpenAi,
            true,
        )?;

        assert_eq!(request["messages"][1]["reasoning_content"], "preserve this");

        Ok(())
    }

    #[test]
    fn test_format_messages_carries_thinking_only_chunks_to_tool_call() -> anyhow::Result<()> {
        let messages = vec![
            Message::assistant().with_content(MessageContentBlock::thinking("think ", "")),
            Message::assistant().with_content(MessageContentBlock::thinking("once", "")),
            Message::assistant().with_tool_request(
                "tool1",
                Ok(CallToolRequestParams::new("test_tool")
                    .with_arguments(object!({"param": "value"}))),
            ),
        ];

        let spec = format_messages_with_options(
            &messages,
            &ImageFormat::OpenAi,
            OpenAiFormatOptions {
                preserve_thinking_context: true,
                ..Default::default()
            },
        );

        assert_eq!(spec.len(), 1);
        assert_eq!(spec[0]["role"], "assistant");
        assert_eq!(spec[0]["reasoning_content"], "think once");
        assert_eq!(spec[0]["content"], json!(null));
        assert_eq!(spec[0]["tool_calls"][0]["function"]["name"], "test_tool");

        Ok(())
    }

    #[test]
    fn test_format_messages_does_not_duplicate_pending_thinking() -> anyhow::Result<()> {
        let messages = vec![
            Message::assistant().with_content(MessageContentBlock::thinking("think once", "")),
            Message::assistant()
                .with_content(MessageContentBlock::thinking("think once", ""))
                .with_tool_request(
                    "tool1",
                    Ok(CallToolRequestParams::new("test_tool")
                        .with_arguments(object!({"param": "value"}))),
                ),
        ];

        let spec = format_messages_with_options(
            &messages,
            &ImageFormat::OpenAi,
            OpenAiFormatOptions {
                preserve_thinking_context: true,
                ..Default::default()
            },
        );

        assert_eq!(spec.len(), 1);
        assert_eq!(spec[0]["reasoning_content"], "think once");
        assert_eq!(spec[0]["tool_calls"][0]["function"]["name"], "test_tool");

        Ok(())
    }

    #[test]
    fn test_format_messages_merges_pending_thinking_with_tool_call_suffix() -> anyhow::Result<()> {
        let messages = vec![
            Message::assistant().with_content(MessageContentBlock::thinking("think ", "")),
            Message::assistant()
                .with_content(MessageContentBlock::thinking("once", ""))
                .with_tool_request(
                    "tool1",
                    Ok(CallToolRequestParams::new("test_tool")
                        .with_arguments(object!({"param": "value"}))),
                ),
        ];

        let spec = format_messages_with_options(
            &messages,
            &ImageFormat::OpenAi,
            OpenAiFormatOptions {
                preserve_thinking_context: true,
                ..Default::default()
            },
        );

        assert_eq!(spec.len(), 1);
        assert_eq!(spec[0]["reasoning_content"], "think once");
        assert_eq!(spec[0]["tool_calls"][0]["function"]["name"], "test_tool");

        Ok(())
    }

    #[test]
    fn test_format_messages_does_not_carry_thinking_across_user_message() -> anyhow::Result<()> {
        let messages = vec![
            Message::assistant().with_content(MessageContentBlock::thinking("stale", "")),
            Message::user().with_text("new turn"),
            Message::assistant()
                .with_tool_request("tool1", Ok(CallToolRequestParams::new("test_tool"))),
        ];

        let spec = format_messages_with_options(
            &messages,
            &ImageFormat::OpenAi,
            OpenAiFormatOptions {
                preserve_thinking_context: true,
                ..Default::default()
            },
        );

        assert_eq!(spec.len(), 2);
        assert_eq!(spec[0]["role"], "user");
        assert_eq!(spec[1]["role"], "assistant");
        assert!(spec[1].get("reasoning_content").is_none());

        Ok(())
    }

    #[test]
    fn test_format_messages_carries_reasoning_through_text_only_chunks() -> anyhow::Result<()> {
        // Scenario B from the streaming bug: thinking arrives first, then multiple
        // text-only assistant messages, then a tool call with thinking re-attached
        // by agent.rs (via the earlier-chunk lookback).
        // Text-only messages set tool_call_turn_reasoning="" (line 453 else-branch),
        // but the TC's own Thinking content must repopulate it.
        let messages = vec![
            Message::assistant().with_content(MessageContentBlock::thinking("reason", "")),
            Message::assistant().with_text("partial answer"),
            Message::assistant().with_text("more text"),
            // agent.rs attaches the earlier thinking to the TC message
            Message::assistant()
                .with_content(MessageContentBlock::thinking("reason", ""))
                .with_tool_request(
                    "tool1",
                    Ok(CallToolRequestParams::new("test_tool").with_arguments(object!({}))),
                ),
        ];

        let spec = format_messages_with_options(
            &messages,
            &ImageFormat::OpenAi,
            OpenAiFormatOptions {
                preserve_thinking_context: true,
                ..Default::default()
            },
        );

        let tool_call_msgs: Vec<_> = spec
            .iter()
            .filter(|m| {
                m.get("tool_calls")
                    .and_then(|tc| tc.as_array())
                    .is_some_and(|a| !a.is_empty())
            })
            .collect();

        assert_eq!(tool_call_msgs.len(), 1);
        assert_eq!(
            tool_call_msgs[0]["reasoning_content"], "reason",
            "reasoning_content must survive text-only chunks between thinking and tool call"
        );

        Ok(())
    }

    #[test]
    fn test_format_messages_carries_reasoning_to_all_split_tool_calls() -> anyhow::Result<()> {
        // Simulates DeepSeek/Kimi streaming: a thinking-only chunk arrives first,
        // then the agent splits two tool calls into separate messages, each with
        // the same reasoning attached (as agent.rs does via response_thinking).
        // The formatter must keep reasoning_content on both so that
        // merge_split_tool_call_messages can reunite them into one assistant message.
        let tool_result1 = Message::user().with_tool_response(
            "tool1",
            Ok(rmcp::model::CallToolResult::success(vec![
                rmcp::model::ContentBlock::text("result1"),
            ])),
        );
        let messages = vec![
            // Standalone thinking message (created by agent.rs alongside request_msgs)
            Message::assistant().with_content(MessageContentBlock::thinking("reasoning", "")),
            // Each request_msg has thinking explicitly attached (agent.rs behaviour)
            Message::assistant()
                .with_content(MessageContentBlock::thinking("reasoning", ""))
                .with_tool_request(
                    "tool1",
                    Ok(CallToolRequestParams::new("tool_a").with_arguments(object!({}))),
                ),
            tool_result1,
            Message::assistant()
                .with_content(MessageContentBlock::thinking("reasoning", ""))
                .with_tool_request(
                    "tool2",
                    Ok(CallToolRequestParams::new("tool_b").with_arguments(object!({}))),
                ),
        ];

        let spec = format_messages_with_options(
            &messages,
            &ImageFormat::OpenAi,
            OpenAiFormatOptions {
                preserve_thinking_context: true,
                ..Default::default()
            },
        );

        // After merge: one assistant message with both tool calls
        let assistant_msgs: Vec<_> = spec
            .iter()
            .filter(|m| m.get("role") == Some(&json!("assistant")))
            .collect();
        assert_eq!(assistant_msgs.len(), 1);
        assert_eq!(assistant_msgs[0]["reasoning_content"], "reasoning");
        let tool_calls = assistant_msgs[0]["tool_calls"].as_array().unwrap();
        assert_eq!(tool_calls.len(), 2);

        Ok(())
    }

    #[test]
    fn test_sequential_tool_calls_not_merged() -> anyhow::Result<()> {
        // Verifies that two tool calls from *different* turns are never merged,
        // even when the second call carries no fresh reasoning (the previous
        // turn's reasoning must not leak into it).
        let tool_result1 = Message::user().with_tool_response(
            "tool1",
            Ok(rmcp::model::CallToolResult::success(vec![
                rmcp::model::ContentBlock::text("result1"),
            ])),
        );
        let messages = vec![
            // Turn 1: thinking then tool call
            Message::assistant().with_content(MessageContentBlock::thinking("turn1_reasoning", "")),
            Message::assistant().with_tool_request(
                "tool1",
                Ok(CallToolRequestParams::new("tool_a").with_arguments(object!({}))),
            ),
            tool_result1,
            // Turn 2: new tool call, no fresh thinking
            Message::assistant().with_tool_request(
                "tool2",
                Ok(CallToolRequestParams::new("tool_b").with_arguments(object!({}))),
            ),
        ];

        let spec = format_messages_with_options(
            &messages,
            &ImageFormat::OpenAi,
            OpenAiFormatOptions {
                preserve_thinking_context: true,
                ..Default::default()
            },
        );

        let assistant_msgs: Vec<_> = spec
            .iter()
            .filter(|m| m.get("role") == Some(&json!("assistant")))
            .collect();

        // Must remain two separate assistant messages — not merged across turns.
        assert_eq!(
            assistant_msgs.len(),
            2,
            "sequential tool calls must not be merged"
        );

        // Turn 1 carries reasoning; turn 2 must not inherit it.
        assert_eq!(assistant_msgs[0]["reasoning_content"], "turn1_reasoning");
        assert!(
            assistant_msgs[1].get("reasoning_content").is_none()
                || assistant_msgs[1]["reasoning_content"].is_null(),
            "turn 2 must not inherit stale reasoning from turn 1"
        );

        // The tool result must appear between the two assistant messages.
        let tool_idx = spec
            .iter()
            .position(|m| m.get("role") == Some(&json!("tool")))
            .expect("tool result must be present");
        let asst1_idx = spec
            .iter()
            .position(|m| m.get("role") == Some(&json!("assistant")))
            .unwrap();
        let asst2_idx = spec
            .iter()
            .rposition(|m| m.get("role") == Some(&json!("assistant")))
            .unwrap();
        assert!(
            asst1_idx < tool_idx && tool_idx < asst2_idx,
            "tool result must sit between the two assistant messages"
        );

        Ok(())
    }

    #[test_case(
        "data: {\"error\":{\"message\":\"Internal server error\",\"type\":\"server_error\",\"code\":500}}\ndata: [DONE]",
        "Internal server error";
        "openai error format"
    )]
    #[test_case(
        "data: {\"object\":\"error\",\"message\":\"CUDA out of memory\",\"code\":500}\ndata: [DONE]",
        "CUDA out of memory";
        "vllm error format"
    )]
    #[test_case(
        "data: {\"error\":{\"message\":\"Rate limit exceeded\",\"type\":\"rate_limit_error\"}}",
        "Rate limit exceeded";
        "error as first chunk"
    )]
    #[tokio::test]
    async fn test_mid_stream_server_error(response_lines: &str, expected_msg: &str) {
        let lines: Vec<String> = response_lines.lines().map(|s| s.to_string()).collect();
        let response_stream = tokio_stream::iter(lines.into_iter().map(Ok));
        let mut messages = std::pin::pin!(response_to_streaming_message(response_stream));
        let mut found_error = false;
        while let Some(result) = messages.next().await {
            if let Err(e) = result {
                let err_str = e.to_string();
                assert!(
                    err_str.contains(expected_msg),
                    "unexpected error text: {err_str}"
                );
                found_error = true;
                break;
            }
        }
        assert!(
            found_error,
            "expected an error but stream completed successfully"
        );
    }

    #[test]
    fn test_merge_split_tool_calls_with_reasoning() {
        let mut messages = vec![
            json!({"role": "assistant", "tool_calls": [{"id": "tc1", "type": "function", "function": {"name": "read", "arguments": "{}"}}], "reasoning_content": "thinking..."}),
            json!({"role": "tool", "tool_call_id": "tc1", "content": "result1"}),
            json!({"role": "assistant", "tool_calls": [{"id": "tc2", "type": "function", "function": {"name": "write", "arguments": "{}"}}], "reasoning_content": "thinking..."}),
            json!({"role": "tool", "tool_call_id": "tc2", "content": "result2"}),
        ];
        merge_split_tool_call_messages(&mut messages);

        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0]["tool_calls"].as_array().unwrap().len(), 2);
        assert_eq!(messages[1]["role"], "tool");
        assert_eq!(messages[2]["role"], "tool");
    }

    #[test]
    fn test_no_merge_without_reasoning() {
        let mut messages = vec![
            json!({"role": "assistant", "tool_calls": [{"id": "tc1", "type": "function", "function": {"name": "read", "arguments": "{}"}}]}),
            json!({"role": "tool", "tool_call_id": "tc1", "content": "result1"}),
            json!({"role": "assistant", "tool_calls": [{"id": "tc2", "type": "function", "function": {"name": "write", "arguments": "{}"}}]}),
            json!({"role": "tool", "tool_call_id": "tc2", "content": "result2"}),
        ];
        merge_split_tool_call_messages(&mut messages);

        assert_eq!(messages.len(), 4);
        assert_eq!(messages[0]["tool_calls"].as_array().unwrap().len(), 1);
        assert_eq!(messages[2]["tool_calls"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn test_merge_split_tool_calls_with_image_gap() {
        let mut messages = vec![
            json!({"role": "assistant", "tool_calls": [{"id": "tc1", "type": "function", "function": {"name": "screenshot", "arguments": "{}"}}], "reasoning_content": "thinking..."}),
            json!({"role": "tool", "tool_call_id": "tc1", "content": "This tool result included an image that is uploaded in the next message."}),
            json!({"role": "user", "content": [{"type": "image_url", "image_url": {"url": "data:image/png;base64,abc"}}]}),
            json!({"role": "assistant", "tool_calls": [{"id": "tc2", "type": "function", "function": {"name": "click", "arguments": "{}"}}], "reasoning_content": "thinking..."}),
            json!({"role": "tool", "tool_call_id": "tc2", "content": "clicked"}),
        ];
        merge_split_tool_call_messages(&mut messages);

        assert_eq!(messages.len(), 4);
        assert_eq!(messages[0]["tool_calls"].as_array().unwrap().len(), 2);
        assert_eq!(messages[0]["role"], "assistant");
        assert_eq!(messages[1]["role"], "tool");
        assert_eq!(messages[1]["tool_call_id"], "tc1");
        assert_eq!(messages[2]["role"], "user");
        assert_eq!(messages[3]["role"], "tool");
        assert_eq!(messages[3]["tool_call_id"], "tc2");
    }

    #[test]
    fn test_merge_does_not_skip_real_user_message() {
        let mut messages = vec![
            json!({"role": "assistant", "tool_calls": [{"id": "tc1", "type": "function", "function": {"name": "read", "arguments": "{}"}}], "reasoning_content": "thinking..."}),
            json!({"role": "tool", "tool_call_id": "tc1", "content": "result1"}),
            json!({"role": "user", "content": "what happened?"}),
            json!({"role": "assistant", "tool_calls": [{"id": "tc2", "type": "function", "function": {"name": "write", "arguments": "{}"}}], "reasoning_content": "thinking..."}),
            json!({"role": "tool", "tool_call_id": "tc2", "content": "result2"}),
        ];
        merge_split_tool_call_messages(&mut messages);

        assert_eq!(messages.len(), 5);
        assert_eq!(messages[0]["tool_calls"].as_array().unwrap().len(), 1);
        assert_eq!(messages[2]["role"], "user");
        assert_eq!(messages[2]["content"], "what happened?");
        assert_eq!(messages[3]["tool_calls"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn test_get_usage_with_ollama_native_fields() {
        // Ollama-native fields should be picked up as fallback
        let usage = json!({
            "prompt_eval_count": 42,
            "eval_count": 128
        });
        let result = get_usage(&usage);
        assert_eq!(result.input_tokens, Some(42));
        assert_eq!(result.output_tokens, Some(128));
        assert_eq!(result.total_tokens, Some(170));
    }

    #[test]
    fn test_get_usage_prefers_openai_fields_over_ollama() {
        // Standard OpenAI fields should take precedence
        let usage = json!({
            "prompt_tokens": 10,
            "completion_tokens": 20,
            "prompt_eval_count": 42,
            "eval_count": 128
        });
        let result = get_usage(&usage);
        assert_eq!(result.input_tokens, Some(10));
        assert_eq!(result.output_tokens, Some(20));
        assert_eq!(result.total_tokens, Some(30));
    }

    #[test]
    fn test_get_usage_falls_back_when_openai_fields_are_null() {
        // When OpenAI fields exist but are null, should fall back to Ollama-native
        let usage = json!({
            "prompt_tokens": null,
            "completion_tokens": null,
            "prompt_eval_count": 42,
            "eval_count": 128
        });
        let result = get_usage(&usage);
        assert_eq!(result.input_tokens, Some(42));
        assert_eq!(result.output_tokens, Some(128));
        assert_eq!(result.total_tokens, Some(170));
    }

    // vLLM serving gpt-oss emits both `reasoning` and `reasoning_content`
    // in the same payload; the non-streaming path handles it fine today.
    #[test]
    fn test_response_to_message_with_both_reasoning_fields() -> anyhow::Result<()> {
        let response = json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "answer",
                    "reasoning": "thinking...",
                    "reasoning_content": "thinking..."
                }
            }]
        });

        let message = response_to_message(&response)?;
        assert_eq!(message.content.len(), 2);
        if let MessageContentBlock::Thinking(t) = &message.content[0] {
            assert_eq!(t.thinking, "thinking...");
        } else {
            panic!("Expected Thinking content, got {:?}", message.content[0]);
        }
        Ok(())
    }

    // ---- metadata-only SSE frames (gateway trace/guardrail objects) -----------------------
    //
    // Some OpenAI-compatible gateways interleave objects that have no `choices` key at all
    // with the real chunks. A Portkey gateway sends a `hook_results` guardrail trace as the
    // FIRST frame whenever strict-openai-compliance is off. Failing on such a frame killed
    // the whole turn.

    /// A guardrail trace frame, in the shape a Portkey gateway emits it.
    const METADATA_FRAME: &str = concat!(
        r#"data: {"hook_results":{"before_request_hooks":[{"verdict":true,"#,
        r#""id":"guardrail-1","type":"guardrail","deny":false}]}}"#
    );

    fn content_chunk(content: &str) -> String {
        format!(
            concat!(
                r#"data: {{"id":"x","object":"chat.completion.chunk","created":1,"model":"m","#,
                r#""choices":[{{"index":0,"delta":{{"content":"{}"}},"finish_reason":null}}]}}"#
            ),
            content
        )
    }

    #[tokio::test]
    async fn test_metadata_only_first_frame_does_not_abort_stream() -> anyhow::Result<()> {
        // The reported bug: a metadata-only opening frame made the whole turn fail with
        // "Failed to parse streaming chunk: missing field `choices`".
        let response_lines = format!("{METADATA_FRAME}\n{}\ndata: [DONE]", content_chunk("hello"));
        assert_eq!(run_streaming_test(&response_lines).await?.text, "hello");
        Ok(())
    }

    #[tokio::test]
    async fn test_metadata_only_frame_interleaved_mid_stream_is_skipped() -> anyhow::Result<()> {
        let response_lines = format!(
            "{}\n{METADATA_FRAME}\n{}\ndata: [DONE]",
            content_chunk("hel"),
            content_chunk("lo")
        );
        assert_eq!(run_streaming_test(&response_lines).await?.text, "hello");
        Ok(())
    }

    #[tokio::test]
    async fn test_metadata_only_frame_mid_tool_call_keeps_arguments_intact() -> anyhow::Result<()> {
        // A metadata frame arriving between two tool_calls argument deltas must not end
        // argument accumulation. Merely defaulting `choices` to an empty vec would route this
        // frame into the inner loop's empty-choices branch (`done = true`) and silently
        // truncate the arguments to `{"city":"Pa` — a quiet corruption instead of a loud error.
        let response_lines = concat!(
            r#"data: {"id":"x","object":"chat.completion.chunk","model":"m","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"get_weather","arguments":"{\"city\":\"Pa"}}]},"finish_reason":null}]}"#,
            "\n",
            r#"data: {"hook_results":{"before_request_hooks":[{"verdict":true,"type":"guardrail","deny":false}]}}"#,
            "\n",
            r#"data: {"id":"x","object":"chat.completion.chunk","model":"m","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"ris\"}"}}]},"finish_reason":null}]}"#,
            "\n",
            r#"data: {"id":"x","object":"chat.completion.chunk","model":"m","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#,
            "\n",
            "data: [DONE]"
        );
        let lines: Vec<String> = response_lines.lines().map(|s| s.to_string()).collect();
        let response_stream = tokio_stream::iter(lines.into_iter().map(Ok));
        let mut messages = std::pin::pin!(response_to_streaming_message(response_stream));

        let mut tool_calls = Vec::new();
        while let Some(result) = messages.next().await {
            let (message, _usage) = result?;
            if let Some(msg) = message {
                for content in &msg.content {
                    if let MessageContentBlock::ToolRequest(req) = content {
                        if let Ok(call) = &req.tool_call {
                            tool_calls.push(call.clone());
                        }
                    }
                }
            }
        }

        assert_eq!(tool_calls.len(), 1, "expected exactly one tool call");
        assert_eq!(tool_calls[0].name, "get_weather");
        assert_eq!(
            tool_calls[0]
                .arguments
                .as_ref()
                .and_then(|a| a.get("city"))
                .and_then(Value::as_str),
            Some("Paris"),
            "arguments must survive the interleaved metadata frame intact"
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_empty_choices_array_is_not_treated_as_metadata() -> anyhow::Result<()> {
        // `"choices": []` is the standard usage-only chunk, NOT a metadata frame: it must
        // still deserialize and still surface its usage.
        let response_lines = concat!(
            r#"data: {"id":"x","object":"chat.completion.chunk","model":"m","choices":[{"index":0,"delta":{"content":"hi"},"finish_reason":"stop"}]}"#,
            "\n",
            r#"data: {"id":"x","object":"chat.completion.chunk","model":"m","choices":[],"usage":{"prompt_tokens":7,"completion_tokens":3,"total_tokens":10}}"#,
            "\n",
            "data: [DONE]"
        );
        let result = run_streaming_test(response_lines).await?;
        assert_eq!(result.usage_count, 1, "the usage-only chunk must be kept");
        let usage = result.usage.expect("usage should be reported");
        assert_eq!(usage.usage.output_tokens, Some(3));
        assert!(result.has_text_content);
        Ok(())
    }

    #[test]
    fn test_error_frames_still_surface_as_server_error() {
        // Skipping choice-less frames must not swallow gateway error frames, which are also
        // choice-less. Every one of these is handled ahead of the metadata skip.
        for line in [
            r#"{"error":{"message":"upstream exploded"}}"#,
            r#"{"object":"error","message":"upstream exploded"}"#,
            // No `error` key and no `object`: the shape Azure APIM rate-limits with, on an
            // HTTP 200. Skipping this would report the failed turn as an empty success.
            r#"{"statusCode":429,"message":"upstream exploded"}"#,
            // Some gateways stringify the status.
            r#"{"status":"503","message":"upstream exploded"}"#,
            // FastAPI's HTTPException shape.
            r#"{"detail":"upstream exploded"}"#,
            // FastAPI's RequestValidationError shape: `detail` is a LIST, so a string-only
            // read would drop it and silently skip the frame.
            r#"{"detail":[{"loc":["body"],"msg":"upstream exploded","type":"value_error"}]}"#,
            // A non-string `message` must not be dropped either.
            r#"{"statusCode":500,"message":{"text":"upstream exploded"}}"#,
            // `type: "error"` is a third error marker some gateways use.
            r#"{"type":"error","message":"upstream exploded"}"#,
        ] {
            match parse_streaming_chunk(line) {
                Err(ProviderError::ServerError(msg)) => assert!(
                    msg.contains("upstream exploded"),
                    "message preserved for {line}, got {msg:?}"
                ),
                other => panic!("expected ServerError for {line}, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn test_informational_choiceless_frame_is_still_skipped() -> anyhow::Result<()> {
        // Prose alone is not an error signal. A keepalive/progress frame carrying a message
        // but no status must NOT abort the turn — doing so would reintroduce exactly the bug
        // the metadata skip exists to fix.
        let response_lines = format!(
            "{}\n{}\ndata: [DONE]",
            r#"data: {"message":"processing"}"#,
            content_chunk("hello")
        );
        assert_eq!(run_streaming_test(&response_lines).await?.text, "hello");
        Ok(())
    }

    #[test]
    fn test_status_only_error_frame_still_fails_loudly() {
        // No message text anywhere: the frame must still surface rather than vanish.
        match parse_streaming_chunk(r#"{"statusCode":503}"#) {
            Err(ProviderError::ServerError(msg)) => {
                assert!(msg.contains("503"), "status should reach the caller: {msg}")
            }
            other => panic!("expected ServerError, got {other:?}"),
        }
    }

    #[test]
    fn test_choiceless_frame_error_text_is_capped() {
        let long = "x".repeat(5_000);
        let line = format!(r#"{{"statusCode":500,"message":"{long}"}}"#);
        match parse_streaming_chunk(&line) {
            Err(ProviderError::ServerError(msg)) => assert!(
                msg.chars().count() <= MAX_STREAM_ERROR_LEN + 1,
                "error text should be truncated, got {} chars",
                msg.chars().count()
            ),
            other => panic!("expected ServerError, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_in_stream_error_frame_aborts_rather_than_ending_empty() {
        // End-to-end counterpart: the turn must fail, not complete with no content. A silent
        // skip here tells the user to resend — the worst possible advice into a 429.
        let response_lines = concat!(
            r#"data: {"statusCode":429,"message":"rate limited"}"#,
            "\n",
            "data: [DONE]"
        );
        let lines: Vec<String> = response_lines.lines().map(|s| s.to_string()).collect();
        let response_stream = tokio_stream::iter(lines.into_iter().map(Ok));
        let mut messages = std::pin::pin!(response_to_streaming_message(response_stream));

        let first = messages.next().await.expect("stream should yield an item");
        let err = first.expect_err("an in-stream error frame must not be skipped");
        assert!(
            err.to_string().contains("rate limited"),
            "the gateway's message must reach the caller: {err}"
        );
    }

    #[test]
    fn test_parse_streaming_chunk_returns_none_for_metadata_frames() {
        // Unit-level counterpart to the streaming tests above.
        let metadata = parse_streaming_chunk(r#"{"hook_results":{"before_request_hooks":[]}}"#)
            .expect("metadata frame must not be an error");
        assert!(metadata.is_none(), "metadata frame should be skipped");

        let real = parse_streaming_chunk(r#"{"choices":[],"usage":{"completion_tokens":1}}"#)
            .expect("usage-only chunk must parse");
        assert!(
            real.is_some(),
            "`choices: []` is a real chunk, not metadata"
        );
    }

    #[tokio::test]
    async fn test_streaming_chunk_with_only_reasoning_content() -> anyhow::Result<()> {
        let response_lines = "data: {\"id\":\"x\",\"object\":\"chat.completion.chunk\",\"model\":\"m\",\"choices\":[{\"index\":0,\"delta\":{\"reasoning_content\":\"hi\"},\"finish_reason\":null}]}\ndata: [DONE]";
        let lines: Vec<String> = response_lines.lines().map(|s| s.to_string()).collect();
        let response_stream = tokio_stream::iter(lines.into_iter().map(Ok));
        let mut messages = std::pin::pin!(response_to_streaming_message(response_stream));
        while let Some(result) = messages.next().await {
            result?;
        }
        Ok(())
    }

    #[tokio::test]
    async fn test_streaming_tool_call_does_not_duplicate_yielded_reasoning() -> anyhow::Result<()> {
        let response_lines = concat!(
            "data: {\"id\":\"x\",\"object\":\"chat.completion.chunk\",\"model\":\"m\",\"choices\":[{\"index\":0,\"delta\":{\"reasoning_content\":\"think \"},\"finish_reason\":null}]}\n",
            "data: {\"id\":\"x\",\"object\":\"chat.completion.chunk\",\"model\":\"m\",\"choices\":[{\"index\":0,\"delta\":{\"reasoning_content\":\"once\"},\"finish_reason\":null}]}\n",
            "data: {\"id\":\"x\",\"object\":\"chat.completion.chunk\",\"model\":\"m\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"test_tool\",\"arguments\":\"\"}}]},\"finish_reason\":null}]}\n",
            "data: {\"id\":\"x\",\"object\":\"chat.completion.chunk\",\"model\":\"m\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n",
            "data: [DONE]"
        );
        let lines: Vec<String> = response_lines.lines().map(|s| s.to_string()).collect();
        let response_stream = tokio_stream::iter(lines.into_iter().map(Ok));
        let mut messages = std::pin::pin!(response_to_streaming_message(response_stream));

        let mut thinking = String::new();
        let mut tool_calls = 0;
        let mut history = Vec::new();
        while let Some(result) = messages.next().await {
            let (message, _usage) = result?;
            if let Some(msg) = message {
                for content in &msg.content {
                    match content {
                        MessageContentBlock::Thinking(t) => thinking.push_str(&t.thinking),
                        MessageContentBlock::ToolRequest(_) => tool_calls += 1,
                        _ => {}
                    }
                }
                history.push(msg);
            }
        }

        assert_eq!(thinking, "think once");
        assert_eq!(tool_calls, 1);

        let spec = format_messages_with_options(
            &history,
            &ImageFormat::OpenAi,
            OpenAiFormatOptions {
                preserve_thinking_context: true,
                ..Default::default()
            },
        );
        assert_eq!(spec.len(), 1);
        assert_eq!(spec[0]["reasoning_content"], "think once");
        assert_eq!(spec[0]["tool_calls"][0]["function"]["name"], "test_tool");

        Ok(())
    }

    #[tokio::test]
    async fn test_streaming_tool_call_merges_yielded_reasoning_with_suffix() -> anyhow::Result<()> {
        let response_lines = concat!(
            "data: {\"id\":\"x\",\"object\":\"chat.completion.chunk\",\"model\":\"m\",\"choices\":[{\"index\":0,\"delta\":{\"reasoning_content\":\"think \"},\"finish_reason\":null}]}\n",
            "data: {\"id\":\"x\",\"object\":\"chat.completion.chunk\",\"model\":\"m\",\"choices\":[{\"index\":0,\"delta\":{\"reasoning_content\":\"once\",\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"test_tool\",\"arguments\":\"\"}}]},\"finish_reason\":null}]}\n",
            "data: {\"id\":\"x\",\"object\":\"chat.completion.chunk\",\"model\":\"m\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n",
            "data: [DONE]"
        );
        let lines: Vec<String> = response_lines.lines().map(|s| s.to_string()).collect();
        let response_stream = tokio_stream::iter(lines.into_iter().map(Ok));
        let mut messages = std::pin::pin!(response_to_streaming_message(response_stream));

        let mut history = Vec::new();
        while let Some(result) = messages.next().await {
            let (message, _usage) = result?;
            if let Some(msg) = message {
                history.push(msg);
            }
        }

        let spec = format_messages_with_options(
            &history,
            &ImageFormat::OpenAi,
            OpenAiFormatOptions {
                preserve_thinking_context: true,
                ..Default::default()
            },
        );
        assert_eq!(spec.len(), 1);
        assert_eq!(spec[0]["reasoning_content"], "think once");
        assert_eq!(spec[0]["tool_calls"][0]["function"]["name"], "test_tool");

        Ok(())
    }

    #[tokio::test]
    async fn test_streaming_tool_call_preserves_unyielded_reasoning() -> anyhow::Result<()> {
        let response_lines = concat!(
            "data: {\"id\":\"x\",\"object\":\"chat.completion.chunk\",\"model\":\"m\",\"choices\":[{\"index\":0,\"delta\":{\"reasoning_content\":\"tool thought\",\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"test_tool\",\"arguments\":\"{}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n",
            "data: [DONE]"
        );
        let lines: Vec<String> = response_lines.lines().map(|s| s.to_string()).collect();
        let response_stream = tokio_stream::iter(lines.into_iter().map(Ok));
        let mut messages = std::pin::pin!(response_to_streaming_message(response_stream));

        let mut thinking = String::new();
        let mut tool_calls = 0;
        while let Some(result) = messages.next().await {
            let (message, _usage) = result?;
            if let Some(msg) = message {
                for content in &msg.content {
                    match content {
                        MessageContentBlock::Thinking(t) => thinking.push_str(&t.thinking),
                        MessageContentBlock::ToolRequest(_) => tool_calls += 1,
                        _ => {}
                    }
                }
            }
        }

        assert_eq!(thinking, "tool thought");
        assert_eq!(tool_calls, 1);
        Ok(())
    }

    #[tokio::test]
    async fn test_streaming_tool_call_without_tool_call_index() -> anyhow::Result<()> {
        let response_lines = concat!(
            "data: {\"id\":\"x\",\"object\":\"chat.completion.chunk\",\"model\":\"m\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"tool_calls\":[{\"id\":\"functions.get_weather:0\",\"type\":\"function\",\"function\":{\"name\":\"get_weather\",\"arguments\":\"{\\\"city\\\": \\\"Paris\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n",
            "data: [DONE]"
        );
        let lines: Vec<String> = response_lines.lines().map(|s| s.to_string()).collect();
        let response_stream = tokio_stream::iter(lines.into_iter().map(Ok));
        let mut messages = std::pin::pin!(response_to_streaming_message(response_stream));

        let mut tool_calls = Vec::new();
        while let Some(result) = messages.next().await {
            let (message, _usage) = result?;
            if let Some(msg) = message {
                for content in &msg.content {
                    if let MessageContentBlock::ToolRequest(request) = content {
                        let tool_call = request.tool_call.as_ref().expect("tool call should parse");
                        tool_calls.push((tool_call.name.to_string(), tool_call.arguments.clone()));
                    }
                }
            }
        }

        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].0, "get_weather");
        assert_eq!(tool_calls[0].1, Some(object!({"city": "Paris"})));
        Ok(())
    }

    #[tokio::test]
    async fn test_streaming_tool_call_dotted_name_passes_through() -> anyhow::Result<()> {
        let response_lines = concat!(
            "data: {\"id\":\"x\",\"object\":\"chat.completion.chunk\",\"model\":\"m\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"tool_calls\":[{\"index\":0,\"id\":\"tc1\",\"type\":\"function\",\"function\":{\"name\":\"ext__db.query\",\"arguments\":\"{\\\"command\\\": \\\"ls\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n",
            "data: [DONE]"
        );
        let lines: Vec<String> = response_lines.lines().map(|s| s.to_string()).collect();
        let response_stream = tokio_stream::iter(lines.into_iter().map(Ok));
        let mut messages = std::pin::pin!(response_to_streaming_message(response_stream));

        let mut tool_calls = Vec::new();
        while let Some(result) = messages.next().await {
            let (message, _usage) = result?;
            if let Some(msg) = message {
                for content in &msg.content {
                    if let MessageContentBlock::ToolRequest(request) = content {
                        let tool_call = request.tool_call.as_ref().expect("tool call should parse");
                        tool_calls.push((tool_call.name.to_string(), tool_call.arguments.clone()));
                    }
                }
            }
        }

        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].0, "ext__db.query");
        assert_eq!(tool_calls[0].1, Some(object!({"command": "ls"})));
        Ok(())
    }

    #[tokio::test]
    async fn test_streaming_tool_call_degenerate_name_passes_through() -> anyhow::Result<()> {
        let response_lines = concat!(
            "data: {\"id\":\"x\",\"object\":\"chat.completion.chunk\",\"model\":\"m\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"tool_calls\":[{\"index\":0,\"id\":\"tc1\",\"type\":\"function\",\"function\":{\"name\":\"???\",\"arguments\":\"{}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n",
            "data: [DONE]"
        );
        let lines: Vec<String> = response_lines.lines().map(|s| s.to_string()).collect();
        let response_stream = tokio_stream::iter(lines.into_iter().map(Ok));
        let mut messages = std::pin::pin!(response_to_streaming_message(response_stream));

        let mut names = Vec::new();
        while let Some(result) = messages.next().await {
            let (message, _usage) = result?;
            if let Some(msg) = message {
                for content in &msg.content {
                    if let MessageContentBlock::ToolRequest(request) = content {
                        names.push(
                            request
                                .tool_call
                                .as_ref()
                                .expect("passes through")
                                .name
                                .to_string(),
                        );
                    }
                }
            }
        }

        assert_eq!(names, vec!["???".to_string()]);
        Ok(())
    }

    #[tokio::test]
    async fn test_streaming_multiple_tool_calls_without_tool_call_index() -> anyhow::Result<()> {
        let response_lines = concat!(
            "data: {\"id\":\"x\",\"object\":\"chat.completion.chunk\",\"model\":\"m\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"tool_calls\":[{\"id\":\"functions.get_weather:0\",\"type\":\"function\",\"function\":{\"name\":\"get_weather\",\"arguments\":\"{\\\"city\\\": \\\"Paris\\\"}\"}},{\"id\":\"functions.get_weather:1\",\"type\":\"function\",\"function\":{\"name\":\"get_weather\",\"arguments\":\"{\\\"city\\\": \\\"Tokyo\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n",
            "data: [DONE]"
        );
        let lines: Vec<String> = response_lines.lines().map(|s| s.to_string()).collect();
        let response_stream = tokio_stream::iter(lines.into_iter().map(Ok));
        let mut messages = std::pin::pin!(response_to_streaming_message(response_stream));

        let mut tool_calls = Vec::new();
        while let Some(result) = messages.next().await {
            let (message, _usage) = result?;
            if let Some(msg) = message {
                for content in &msg.content {
                    if let MessageContentBlock::ToolRequest(request) = content {
                        let tool_call = request.tool_call.as_ref().expect("tool call should parse");
                        tool_calls.push((tool_call.name.to_string(), tool_call.arguments.clone()));
                    }
                }
            }
        }

        assert_eq!(tool_calls.len(), 2);
        assert_eq!(tool_calls[0].0, "get_weather");
        assert_eq!(tool_calls[0].1, Some(object!({"city": "Paris"})));
        assert_eq!(tool_calls[1].0, "get_weather");
        assert_eq!(tool_calls[1].1, Some(object!({"city": "Tokyo"})));
        Ok(())
    }

    // Streaming counterpart: both fields in one delta must parse and yield
    // thinking content, not fail with "duplicate field `reasoning_content`".
    #[tokio::test]
    async fn test_streaming_chunk_with_both_reasoning_fields() -> anyhow::Result<()> {
        let response_lines = "data: {\"id\":\"x\",\"object\":\"chat.completion.chunk\",\"model\":\"m\",\"choices\":[{\"index\":0,\"delta\":{\"reasoning\":\"thinking...\",\"reasoning_content\":\"thinking...\"},\"finish_reason\":null}]}\ndata: [DONE]";
        let lines: Vec<String> = response_lines.lines().map(|s| s.to_string()).collect();
        let response_stream = tokio_stream::iter(lines.into_iter().map(Ok));
        let mut messages = std::pin::pin!(response_to_streaming_message(response_stream));

        let mut saw_thinking = false;
        while let Some(result) = messages.next().await {
            let (message, _usage) = result?;
            if let Some(msg) = message {
                for c in &msg.content {
                    if let MessageContentBlock::Thinking(t) = c {
                        assert_eq!(t.thinking, "thinking...");
                        saw_thinking = true;
                    }
                }
            }
        }
        assert!(
            saw_thinking,
            "expected thinking content from merged reasoning fields"
        );
        Ok(())
    }

    #[test]
    fn test_delta_tool_call_function_accepts_null_arguments() {
        let raw = r#"{"arguments":null}"#;
        let parsed: DeltaToolCallFunction =
            serde_json::from_str(raw).expect("null arguments must deserialize");
        assert_eq!(parsed.arguments, "");

        let raw = r#"{}"#;
        let parsed: DeltaToolCallFunction =
            serde_json::from_str(raw).expect("missing arguments must deserialize");
        assert_eq!(parsed.arguments, "");

        let raw = r#"{"arguments":"{\"k\":1}"}"#;
        let parsed: DeltaToolCallFunction = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.arguments, "{\"k\":1}");
    }

    #[test]
    fn test_is_openai_responses_model_matches_openai_reasoning_families() {
        for model in [
            "o3",
            "o3-mini",
            "o4-mini",
            "gpt-5",
            "gpt-5-pro",
            "gpt-5.4",
            "gpt-5.4-mini",
            "gpt-5-4",
            "gpt-5-2-pro",
            "databricks-gpt-5.4",
            "goose-gpt-5.4-high",
            "gpt-6-astra",
            "data_workflow_tools.goose.goose-gpt-6-astra",
            "headless-goose-o3-mini",
        ] {
            assert!(is_openai_responses_model(model), "{model} should match");
        }
    }

    #[test]
    fn test_is_openai_responses_model_rejects_other_families() {
        for model in [
            "gpt-4o",
            "claude-sonnet-4",
            "databricks-claude-sonnet-4",
            "llama-3-70b",
        ] {
            assert!(
                !is_openai_responses_model(model),
                "{model} should not match"
            );
        }
    }

    #[test]
    fn test_xai_reasoning_model_capabilities_are_model_specific() {
        for model in ["grok-4.5", "grok-4.3", "grok-3-mini"] {
            assert!(supports_xai_reasoning_effort(model), "{model}");
            assert!(is_xai_reasoning_model(model), "{model}");
        }

        for model in [
            "grok-4.20",
            "grok-4.20-0309-reasoning",
            "grok-4.20-multi-agent",
            "grok-4-0709",
            "grok-4-fast-reasoning",
        ] {
            assert!(!supports_xai_reasoning_effort(model), "{model}");
            assert!(is_xai_reasoning_model(model), "{model}");
        }

        for model in [
            "grok-4.20-0309-non-reasoning",
            "grok-4-fast-non-reasoning",
            "grok-3",
            "grok-build-0.1",
        ] {
            assert!(!supports_xai_reasoning_effort(model), "{model}");
            assert!(!is_xai_reasoning_model(model), "{model}");
        }
    }

    #[test]
    fn test_xai_reasoning_effort_uses_each_models_supported_levels() {
        assert_eq!(
            xai_reasoning_effort_for_thinking("grok-4.5", ThinkingEffort::Off),
            Some("low".to_string())
        );
        assert_eq!(
            xai_reasoning_effort_for_thinking("grok-4.5", ThinkingEffort::Medium),
            Some("medium".to_string())
        );
        assert_eq!(
            xai_reasoning_effort_for_thinking("grok-4.3", ThinkingEffort::Off),
            Some("none".to_string())
        );
        assert_eq!(
            xai_reasoning_effort_for_thinking("grok-3-mini", ThinkingEffort::Medium),
            Some("high".to_string())
        );
        assert_eq!(
            xai_reasoning_effort_for_thinking("grok-4.5", ThinkingEffort::Max),
            Some("high".to_string())
        );
    }

    #[test]
    fn test_create_request_applies_grok_4_5_reasoning_controls() {
        let model_config = ModelConfig::new("grok-4.5")
            .with_thinking_effort(ThinkingEffort::Medium)
            .with_temperature(Some(0.7));

        let payload = create_request(
            &model_config,
            "system prompt",
            &[],
            &[],
            &ImageFormat::OpenAi,
            false,
        )
        .unwrap();

        assert_eq!(payload.get("reasoning_effort"), Some(&json!("medium")));
        assert!(payload.get("temperature").is_none());
    }

    #[test]
    fn test_create_request_maps_xai_off_to_supported_effort() {
        for (model, expected) in [("grok-4.5", "low"), ("grok-4.3", "none")] {
            let model_config = ModelConfig::new(model).with_thinking_effort(ThinkingEffort::Off);
            let payload = create_request(
                &model_config,
                "system prompt",
                &[],
                &[],
                &ImageFormat::OpenAi,
                false,
            )
            .unwrap();

            assert_eq!(
                payload.get("reasoning_effort"),
                Some(&json!(expected)),
                "{model}"
            );
        }
    }

    #[test]
    fn test_create_request_keeps_grok_4_20_fixed_reasoning_parameters() {
        let model_config = ModelConfig::new("grok-4.20-0309-reasoning")
            .with_thinking_effort(ThinkingEffort::High)
            .with_temperature(Some(0.7));

        let payload = create_request(
            &model_config,
            "system prompt",
            &[],
            &[],
            &ImageFormat::OpenAi,
            false,
        )
        .unwrap();

        assert!(payload.get("reasoning_effort").is_none());
        assert_eq!(payload.get("temperature"), Some(&json!(0.7_f32)));
    }

    #[test]
    fn test_extract_reasoning_effort_for_responses_models() {
        for (model, expected_name, expected_effort) in [
            ("o3-none", "o3", Some("none")),
            ("o3-xhigh", "o3", Some("xhigh")),
            ("gpt-5-low", "gpt-5", Some("low")),
            ("gpt-5.4", "gpt-5.4", None),
            (
                "databricks-gpt-5.4-high",
                "databricks-gpt-5.4",
                Some("high"),
            ),
            ("databricks-o3-low", "databricks-o3", Some("low")),
            ("goose-gpt-5-high", "goose-gpt-5", Some("high")),
            ("gpt-4o", "gpt-4o", None),
        ] {
            let (name, effort) = extract_reasoning_effort(model);
            assert_eq!(name, expected_name, "unexpected base model for {model}");
            assert_eq!(
                effort.as_deref(),
                expected_effort,
                "unexpected effort for {model}"
            );
        }
    }

    #[test]
    fn test_sanitize_function_name() {
        assert_eq!(sanitize_function_name("hello-world"), "hello-world");
        assert_eq!(sanitize_function_name("hello world"), "hello_world");
        assert_eq!(sanitize_function_name("hello@world"), "hello_world");
        assert_eq!(
            sanitize_function_name(&"a".repeat(MAX_FUNCTION_NAME_LENGTH)),
            "a".repeat(MAX_FUNCTION_NAME_LENGTH)
        );
        assert_eq!(
            sanitize_function_name(&"a".repeat(MAX_FUNCTION_NAME_LENGTH + 32)),
            "a".repeat(MAX_FUNCTION_NAME_LENGTH)
        );
    }

    #[test]
    fn test_is_valid_function_name() {
        assert!(is_valid_function_name("hello-world"));
        assert!(is_valid_function_name("hello_world"));
        assert!(is_valid_function_name(
            &"a".repeat(MAX_FUNCTION_NAME_LENGTH + 1)
        ));
        assert!(!is_valid_function_name("hello world"));
        assert!(!is_valid_function_name("hello@world"));
    }

    #[test]
    fn formatter_post_parse_error_history_is_wellformed() {
        use rmcp::model::{ErrorCode, ErrorData};
        let err = ErrorData::new(
            ErrorCode::INVALID_PARAMS,
            "Tool arguments for id call_bad must be a JSON object".to_string(),
            None,
        );
        // Shape the agent loop builds today for a failed parse:
        let request_msg = Message::assistant().with_tool_request("call_bad", Err(err.clone()));
        let mut final_resp = Message::user();
        final_resp.add_tool_response_with_metadata("call_bad", Err(err), None);
        let messages = vec![
            Message::user().with_text("do the thing"),
            request_msg,
            final_resp,
        ];

        let spec = format_messages(&messages, &ImageFormat::OpenAi);

        let mut open = std::collections::HashSet::new();
        for m in &spec {
            match m.get("role").and_then(|v| v.as_str()) {
                Some("assistant") => {
                    for tc in m
                        .get("tool_calls")
                        .and_then(|v| v.as_array())
                        .into_iter()
                        .flatten()
                    {
                        if let Some(id) = tc.get("id").and_then(|v| v.as_str()) {
                            open.insert(id.to_string());
                        }
                    }
                }
                Some("tool") => {
                    let id = m.get("tool_call_id").and_then(|v| v.as_str()).unwrap_or("");
                    assert!(open.contains(id), "orphan role:tool message for id {id:?}");
                }
                _ => {}
            }
        }
    }

    fn format_with_preservation(
        messages: &[Message],
        format: ThinkingPreservationFormat,
    ) -> Vec<Value> {
        format_messages_with_options(
            messages,
            &ImageFormat::OpenAi,
            OpenAiFormatOptions {
                preserve_thinking_context: true,
                supports_vision: false,
                thinking_preservation_format: Some(format),
            },
        )
    }

    #[test]
    fn test_thinking_preservation_content_prepend() {
        let message = Message::assistant()
            .with_thinking("Thinking process", "")
            .with_text("Hello");

        let spec = format_with_preservation(
            std::slice::from_ref(&message),
            ThinkingPreservationFormat::ContentPrepend,
        );

        assert_eq!(spec.len(), 1);
        assert_eq!(spec[0]["content"], json!("Thinking process\n\nHello"));
        assert!(spec[0].get("reasoning_content").is_none());
    }

    #[test]
    fn test_thinking_preservation_content_xml() {
        let message = Message::assistant()
            .with_thinking("Thinking process", "")
            .with_text("Hello");

        let spec = format_with_preservation(
            std::slice::from_ref(&message),
            ThinkingPreservationFormat::ContentXml,
        );

        assert_eq!(spec.len(), 1);
        assert_eq!(
            spec[0]["content"],
            json!("<think>\nThinking process\n</think>\n\nHello")
        );
        assert!(spec[0].get("reasoning_content").is_none());
    }

    #[test]
    fn test_thinking_preservation_reasoning_content_is_unchanged() {
        let message = Message::assistant()
            .with_thinking("Thinking process", "")
            .with_text("Hello");

        let spec = format_with_preservation(
            std::slice::from_ref(&message),
            ThinkingPreservationFormat::ReasoningContent,
        );

        assert_eq!(spec.len(), 1);
        assert_eq!(spec[0]["content"], json!("Hello"));
        assert_eq!(spec[0]["reasoning_content"], json!("Thinking process"));
    }

    #[test]
    fn test_thinking_preservation_runs_after_split_tool_call_merge() {
        // Split tool-call messages are reunited by matching reasoning_content, so
        // inlining must happen afterwards or the merge silently stops working.
        let messages = vec![
            Message::assistant().with_thinking("reasoning", ""),
            Message::assistant()
                .with_thinking("reasoning", "")
                .with_tool_request(
                    "tool1",
                    Ok(CallToolRequestParams::new("tool_a").with_arguments(object!({}))),
                ),
            Message::user().with_tool_response(
                "tool1",
                Ok(rmcp::model::CallToolResult::success(vec![
                    ContentBlock::text("result1"),
                ])),
            ),
            Message::assistant()
                .with_thinking("reasoning", "")
                .with_tool_request(
                    "tool2",
                    Ok(CallToolRequestParams::new("tool_b").with_arguments(object!({}))),
                ),
        ];

        let spec = format_with_preservation(&messages, ThinkingPreservationFormat::ContentXml);

        let assistant: Vec<_> = spec
            .iter()
            .filter(|m| m.get("role") == Some(&json!("assistant")))
            .collect();

        assert_eq!(assistant.len(), 1);
        assert_eq!(assistant[0]["tool_calls"].as_array().unwrap().len(), 2);
        assert!(assistant[0].get("reasoning_content").is_none());
        assert_eq!(
            assistant[0]["content"],
            json!("<think>\nreasoning\n</think>")
        );
    }

    mod cache_prefix_stability {
        use super::*;

        fn turn_context(time: &str, turn_budget: &str) -> String {
            format!(
                "<turn-context>\n\
                 <current-time>{time}</current-time>\n\
                 <working-directory>/Users/me/code/goose</working-directory>\n\
                 <turn-budget>{turn_budget}</turn-budget>\n\
                 </turn-context>"
            )
        }

        fn tool_loop_conversation(turn_context_block: &str) -> Vec<Message> {
            vec![
                Message::user().with_text("What does the main entrypoint do?"),
                Message::assistant().with_text("Let me read it."),
                Message::user()
                    .with_text("Now add error handling to it.")
                    .with_text(turn_context_block),
                Message::assistant().with_tool_request(
                    "tool_1",
                    Ok(CallToolRequestParams::new("read_file")
                        .with_arguments(object!({"path": "src/main.rs"}))),
                ),
                Message::user().with_tool_response(
                    "tool_1",
                    Ok(CallToolResult::success(vec![ContentBlock::text(
                        "fn main() { run(); }",
                    )])),
                ),
            ]
        }

        #[test]
        fn turn_context_stays_on_its_source_message() {
            let block = turn_context("2026-08-03 12:00:00", "14/40 used");
            let spec = format_messages(&tool_loop_conversation(&block), &ImageFormat::OpenAi);

            let occurrences: Vec<usize> = spec
                .iter()
                .enumerate()
                .filter_map(|(i, m)| {
                    m["content"]
                        .as_str()
                        .is_some_and(|c| c.contains("<turn-context>"))
                        .then_some(i)
                })
                .collect();
            assert_eq!(
                occurrences,
                vec![2],
                "turn-context must appear exactly once, on its source message"
            );
        }
    }
}
