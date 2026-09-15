use crate::conversation::message::{Message, MessageContentBlock};
use crate::conversation::token_usage::{ProviderUsage, Usage};
use crate::documents::{
    convert_document, document_media_type_is_supported, unsupported_document_text, DocumentFormat,
    ASSISTANT_ROLE_REASON, UNSUPPORTED_MEDIA_TYPE_REASON,
};
use crate::errors::ProviderError;
use crate::formats::openai::{
    extract_reasoning_effort, is_openai_responses_model, openai_reasoning_effort_for_thinking,
    sanitize_function_name,
};
use crate::mcp_utils::extract_text_from_resource;
use crate::model::ModelConfig;
use crate::utils::{sanitize_unicode_tags, strip_unicode_tags};
use anyhow::{anyhow, Error};
use async_stream::try_stream;
use chrono;
use futures::Stream;
use rmcp::model::{object, CallToolRequestParams, ContentBlock, Role, Tool};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;

#[derive(Debug, Serialize, Deserialize)]
pub struct ResponsesApiResponse {
    pub id: String,
    pub object: String,
    pub created_at: i64,
    pub status: String,
    pub model: String,
    pub output: Vec<ResponseOutputItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<ResponseReasoningInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<ResponseUsage>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub struct SummaryText {
    pub text: String,
}

fn reasoning_from_summary(summary: &[SummaryText]) -> Option<MessageContentBlock> {
    let text: String = summary
        .iter()
        .map(|s| sanitize_unicode_tags(&s.text))
        .collect::<Vec<_>>()
        .join("\n");
    if text.is_empty() {
        None
    } else {
        Some(MessageContentBlock::thinking(text, ""))
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
pub enum ResponseOutputItem {
    Reasoning {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default)]
        summary: Vec<SummaryText>,
    },
    Message {
        // `id` and `status` are required when the OpenAI API emits these
        // items, but Codex rollout files (which reuse the same shape on
        // disk) sometimes omit them. Keep deserialization permissive.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<String>,
        role: String,
        content: Vec<ResponseContentBlock>,
    },
    FunctionCall {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        call_id: Option<String>,
        name: String,
        arguments: String,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
pub enum ResponseContentBlock {
    OutputText {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        annotations: Option<Vec<Value>>,
    },
    Refusal {
        refusal: String,
    },
    ReasoningText {
        text: String,
    },
    ToolCall {
        id: String,
        name: String,
        input: Value,
    },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ResponseReasoningInfo {
    pub effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ResponseIncompleteDetails {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

fn is_output_token_limit_incomplete_reason(reason: &str) -> bool {
    matches!(reason, "max_output_tokens" | "max_tokens")
}

fn response_reached_output_token_limit(
    status: &str,
    incomplete_details: Option<&ResponseIncompleteDetails>,
) -> bool {
    status == "incomplete"
        && incomplete_details
            .and_then(|details| details.reason.as_deref())
            .is_some_and(is_output_token_limit_incomplete_reason)
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ResponseUsage {
    pub input_tokens: i32,
    pub output_tokens: i32,
    pub total_tokens: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens_details: Option<InputTokensDetails>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct InputTokensDetails {
    #[serde(default)]
    pub cached_tokens: Option<i32>,
}

impl ResponseUsage {
    fn to_usage(&self) -> Usage {
        // input_tokens already includes cached tokens
        let cached_tokens = self
            .input_tokens_details
            .as_ref()
            .and_then(|d| d.cached_tokens);
        Usage::new(
            Some(self.input_tokens),
            Some(self.output_tokens),
            Some(self.total_tokens),
        )
        .with_cache_tokens(cached_tokens, None)
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
pub enum ResponsesStreamEvent {
    #[serde(rename = "response.created")]
    ResponseCreated {
        sequence_number: i32,
        response: ResponseMetadata,
    },
    #[serde(rename = "response.in_progress")]
    ResponseInProgress {
        sequence_number: i32,
        response: ResponseMetadata,
    },
    #[serde(rename = "response.output_item.added")]
    OutputItemAdded {
        sequence_number: i32,
        output_index: i32,
        item: ResponseOutputItemInfo,
    },
    #[serde(rename = "response.content_part.added")]
    ContentBlockPartAdded {
        sequence_number: i32,
        item_id: String,
        output_index: i32,
        content_index: i32,
        part: ContentBlockPart,
    },
    #[serde(rename = "response.output_text.delta")]
    OutputTextDelta {
        sequence_number: i32,
        item_id: String,
        output_index: i32,
        content_index: i32,
        delta: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        logprobs: Option<Vec<Value>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        obfuscation: Option<String>,
    },
    #[serde(rename = "response.output_item.done")]
    OutputItemDone {
        sequence_number: i32,
        output_index: i32,
        item: ResponseOutputItemInfo,
    },
    #[serde(rename = "response.content_part.done")]
    ContentBlockPartDone {
        sequence_number: i32,
        item_id: String,
        output_index: i32,
        content_index: i32,
        part: ContentBlockPart,
    },
    #[serde(rename = "response.output_text.done")]
    OutputTextDone {
        sequence_number: i32,
        item_id: String,
        output_index: i32,
        content_index: i32,
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        logprobs: Option<Vec<Value>>,
    },
    #[serde(rename = "response.completed")]
    ResponseCompleted {
        sequence_number: i32,
        response: ResponseMetadata,
    },
    #[serde(rename = "response.incomplete")]
    ResponseIncomplete {
        sequence_number: i32,
        response: ResponseMetadata,
    },
    #[serde(rename = "response.failed")]
    ResponseFailed { sequence_number: i32, error: Value },
    #[serde(rename = "response.function_call_arguments.delta")]
    FunctionCallArgumentsDelta {
        sequence_number: i32,
        item_id: String,
        output_index: i32,
        delta: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        obfuscation: Option<String>,
    },
    #[serde(rename = "response.function_call_arguments.done")]
    FunctionCallArgumentsDone {
        sequence_number: i32,
        item_id: String,
        output_index: i32,
        arguments: String,
    },
    #[serde(rename = "response.refusal.delta")]
    RefusalDelta {
        sequence_number: i32,
        item_id: String,
        output_index: i32,
        content_index: i32,
        delta: String,
    },
    #[serde(rename = "response.refusal.done")]
    RefusalDone {
        sequence_number: i32,
        item_id: String,
        output_index: i32,
        content_index: i32,
        refusal: String,
    },
    #[serde(rename = "error")]
    Error { error: Value },
    #[serde(rename = "keepalive")]
    Keepalive {
        #[serde(default)]
        sequence_number: Option<i32>,
    },
}

fn is_known_responses_stream_event_type(event_type: &str) -> bool {
    matches!(
        event_type,
        "response.created"
            | "response.in_progress"
            | "response.output_item.added"
            | "response.content_part.added"
            | "response.output_text.delta"
            | "response.output_item.done"
            | "response.content_part.done"
            | "response.output_text.done"
            | "response.completed"
            | "response.incomplete"
            | "response.failed"
            | "response.function_call_arguments.delta"
            | "response.function_call_arguments.done"
            | "response.refusal.delta"
            | "response.refusal.done"
            | "error"
            | "keepalive"
    )
}

fn parse_responses_stream_event(data_line: &str) -> anyhow::Result<Option<ResponsesStreamEvent>> {
    let raw_event: Value = serde_json::from_str(data_line).map_err(|e| {
        ProviderError::stream_decode_error(format!(
            "Failed to parse Responses stream event: {}: {:?}",
            e, data_line
        ))
    })?;

    let Some(event_type) = raw_event.get("type").and_then(Value::as_str) else {
        return Ok(None);
    };

    if !is_known_responses_stream_event_type(event_type) {
        return Ok(None);
    }

    let event = serde_json::from_value(raw_event).map_err(|e| {
        ProviderError::stream_decode_error(format!(
            "Failed to parse Responses stream event: {}: {:?}",
            e, data_line
        ))
    })?;
    Ok(Some(event))
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ResponseMetadata {
    pub id: String,
    pub object: String,
    pub created_at: i64,
    pub status: String,
    pub model: String,
    #[serde(default)]
    pub output: Vec<ResponseOutputItemInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<ResponseUsage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<ResponseReasoningInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub incomplete_details: Option<ResponseIncompleteDetails>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
pub enum ResponseOutputItemInfo {
    Reasoning {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default)]
        summary: Vec<SummaryText>,
    },
    Message {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        status: Option<String>,
        role: String,
        content: Vec<ContentBlockPart>,
    },
    FunctionCall {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        call_id: Option<String>,
        name: String,
        arguments: String,
    },
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
pub enum ContentBlockPart {
    OutputText {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        annotations: Option<Vec<Value>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        logprobs: Option<Vec<Value>>,
    },
    Refusal {
        refusal: String,
    },
    ReasoningText {
        text: String,
    },
    ToolCall {
        id: String,
        name: String,
        arguments: String,
    },
}

fn add_message_items(input_items: &mut Vec<Value>, messages: &[Message], supports_vision: bool) {
    for message in messages.iter().filter(|m| m.is_agent_visible()) {
        let role = match message.role {
            Role::User => "user",
            Role::Assistant => "assistant",
        };

        let mut text_items = Vec::new();

        for content in &message.content {
            match content {
                MessageContentBlock::Text(text) if !text.text.is_empty() => {
                    if message.role == Role::Assistant {
                        // Responses output_text items require annotations even when empty.
                        text_items.push(json!({
                            "type": "output_text",
                            "text": text.text,
                            "annotations": []
                        }));
                    } else {
                        text_items.push(json!({
                            "type": "input_text",
                            "text": text.text
                        }));
                    }
                }
                MessageContentBlock::ToolRequest(request) if message.role == Role::Assistant => {
                    if !text_items.is_empty() {
                        input_items.push(json!({
                            "type": "message",
                            "role": role,
                            "content": text_items
                        }));
                        text_items = Vec::new();
                    }

                    match &request.tool_call {
                        Ok(tool_call) => {
                            let sanitized_name = sanitize_function_name(&tool_call.name);
                            let arguments_str = tool_call
                                .arguments
                                .as_ref()
                                .map(|args| {
                                    serde_json::to_string(args).unwrap_or_else(|_| "{}".to_string())
                                })
                                .unwrap_or_else(|| "{}".to_string());

                            tracing::debug!(
                                "Replaying function_call with call_id: {}, name: {}",
                                request.id,
                                tool_call.name
                            );
                            input_items.push(json!({
                                "type": "function_call",
                                "call_id": request.id,
                                "name": sanitized_name,
                                "arguments": arguments_str
                            }));
                        }
                        Err(e) => {
                            input_items.push(json!({
                                "type": "function_call_output",
                                "call_id": request.id,
                                "output": format!("Error: {}", e.message)
                            }));
                        }
                    }
                }
                MessageContentBlock::Image(image) => {
                    if supports_vision {
                        text_items.push(json!({
                            "type": "input_image",
                            "image_url": format!("data:{};base64,{}", image.mime_type, image.data)
                        }));
                    } else {
                        text_items.push(json!({
                            "type": "input_text",
                            "text": "[image omitted: model does not support vision]"
                        }));
                    }
                }
                MessageContentBlock::Document(document) => {
                    if message.role != Role::User {
                        text_items.push(json!({
                            "type": "output_text",
                            "text": unsupported_document_text(document, ASSISTANT_ROLE_REASON),
                            "annotations": []
                        }));
                    } else if document_media_type_is_supported(&document.mime_type) {
                        let mut converted = convert_document(document, &DocumentFormat::OpenAi);
                        let mut file = converted["file"].take();
                        file["type"] = json!("input_file");
                        text_items.push(file);
                    } else {
                        text_items.push(json!({
                            "type": "input_text",
                            "text": unsupported_document_text(document, UNSUPPORTED_MEDIA_TYPE_REASON)
                        }));
                    }
                }
                MessageContentBlock::ToolResponse(response) => {
                    if !text_items.is_empty() {
                        input_items.push(json!({
                            "type": "message",
                            "role": role,
                            "content": text_items
                        }));
                        text_items = Vec::new();
                    }

                    match &response.tool_result {
                        Ok(contents) => {
                            let has_images = supports_vision
                                && contents
                                    .content
                                    .iter()
                                    .any(|c| matches!(c, ContentBlock::Image(_)));

                            let output = if has_images {
                                json!(contents
                                    .content
                                    .iter()
                                    .map(|c| match c {
                                        ContentBlock::Text(t) => json!({
                                            "type": "input_text", "text": t.text
                                        }),
                                        ContentBlock::Resource(r) => json!({
                                            "type": "input_text",
                                            "text": extract_text_from_resource(&r.resource)
                                        }),
                                        ContentBlock::Image(image) => json!({
                                            "type": "input_image",
                                            "image_url": format!(
                                                "data:{};base64,{}",
                                                image.mime_type, image.data
                                            )
                                        }),
                                        ContentBlock::Audio(_) => json!({
                                            "type": "input_text", "text": "[Audio content]"
                                        }),
                                        ContentBlock::ResourceLink(_) => json!({
                                            "type": "input_text", "text": "[Resource link]"
                                        }),
                                        _ => json!({
                                            "type": "input_text", "text": "[Unsupported content]"
                                        }),
                                    })
                                    .collect::<Vec<Value>>())
                            } else {
                                json!(contents
                                    .content
                                    .iter()
                                    .map(|c| match c {
                                        ContentBlock::Text(t) => t.text.clone(),
                                        ContentBlock::Resource(r) => {
                                            extract_text_from_resource(&r.resource)
                                        }
                                        ContentBlock::Audio(_) => "[Audio content]".into(),
                                        ContentBlock::ResourceLink(_) => {
                                            "[Resource link]".into()
                                        }
                                        ContentBlock::Image(_) =>
                                            "[image omitted: model does not support vision]".into(),
                                        _ => "[Unsupported content]".into(),
                                    })
                                    .collect::<Vec<String>>()
                                    .join("\n"))
                            };

                            input_items.push(json!({
                                "type": "function_call_output",
                                "call_id": response.id,
                                "output": output
                            }));
                        }
                        Err(error_data) => {
                            tracing::debug!(
                                "Sending function_call_output error with call_id: {}",
                                response.id
                            );
                            input_items.push(json!({
                                "type": "function_call_output",
                                "call_id": response.id,
                                "output": format!("Error: {}", error_data.message)
                            }));
                        }
                    }
                }
                _ => {}
            }
        }

        if !text_items.is_empty() {
            input_items.push(json!({
                "type": "message",
                "role": role,
                "content": text_items
            }));
        }
    }
}

fn is_gpt_5_6_model(model_name: &str) -> bool {
    let normalized = model_name.to_ascii_lowercase();
    ["gpt-5.6", "gpt-5-6"].iter().any(|needle| {
        normalized.match_indices(needle).any(|(start, matched)| {
            let before = start
                .checked_sub(1)
                .and_then(|index| normalized.as_bytes().get(index));
            let after = normalized.as_bytes().get(start + matched.len());

            before.is_none_or(|byte| matches!(byte, b'-' | b'/'))
                && after.is_none_or(|byte| matches!(byte, b'-' | b'/'))
        })
    })
}

pub fn create_responses_request(
    model_config: &ModelConfig,
    system: &str,
    messages: &[Message],
    tools: &[Tool],
) -> anyhow::Result<Value, Error> {
    let (wire_model_name, _) = extract_reasoning_effort(&model_config.model_name);
    create_responses_request_for_model(
        model_config,
        &wire_model_name,
        &model_config.model_name,
        system,
        messages,
        tools,
    )
}

pub fn create_responses_request_for_model(
    model_config: &ModelConfig,
    wire_model_name: &str,
    capability_model_name: &str,
    system: &str,
    messages: &[Message],
    tools: &[Tool],
) -> anyhow::Result<Value, Error> {
    let mut input_items = Vec::new();

    if !system.is_empty() {
        input_items.push(json!({
            "type": "message",
            "role": "system",
            "content": [{
                "type": "input_text",
                "text": system
            }]
        }));
    }

    add_message_items(
        &mut input_items,
        messages,
        model_config.supports_vision.unwrap_or_default(),
    );

    let (model_name, legacy_reasoning_effort) = extract_reasoning_effort(capability_model_name);
    // All models routed here are responses-capable; temperature is rejected
    // by the API for reasoning models regardless of whether an explicit
    // effort suffix was provided.
    let is_reasoning_model = is_openai_responses_model(&model_name);
    let reasoning_effort = if is_reasoning_model {
        if let Some(effort) = legacy_reasoning_effort.as_deref() {
            if effort.eq_ignore_ascii_case("none") {
                legacy_reasoning_effort
            } else {
                effort
                    .parse()
                    .ok()
                    .and_then(|effort| openai_reasoning_effort_for_thinking(&model_name, effort))
                    .or(legacy_reasoning_effort)
            }
        } else {
            model_config
                .thinking_effort()
                .and_then(|effort| openai_reasoning_effort_for_thinking(&model_name, effort))
        }
    } else {
        None
    };

    let store = model_config.request_param::<bool>("store").unwrap_or(false);
    let reasoning_mode = model_config
        .request_param::<String>("reasoning_mode")
        .map(|mode| {
            let normalized = mode.to_ascii_lowercase();
            match normalized.as_str() {
                "standard" | "pro" => Ok(normalized),
                _ => Err(anyhow!(
                    "Invalid reasoning_mode '{}'. Supported values are: standard, pro",
                    mode
                )),
            }
        })
        .transpose()?;
    if reasoning_mode.is_some() && !is_gpt_5_6_model(&model_name) {
        return Err(anyhow!(
            "reasoning_mode is only supported for GPT-5.6 models"
        ));
    }
    let mut payload = json!({
        "model": wire_model_name,
        "input": input_items,
        "store": store,
    });

    if reasoning_effort.is_some() || reasoning_mode.is_some() {
        let mut reasoning = serde_json::Map::new();
        if let Some(effort) = reasoning_effort {
            reasoning.insert("effort".to_string(), json!(effort));
            reasoning.insert("summary".to_string(), json!("auto"));
        }
        if let Some(mode) = reasoning_mode {
            reasoning.insert("mode".to_string(), json!(mode));
        }
        payload
            .as_object_mut()
            .unwrap()
            .insert("reasoning".to_string(), Value::Object(reasoning));
    }

    if !tools.is_empty() {
        let tools_spec: Vec<Value> = tools
            .iter()
            .map(|tool| {
                json!({
                    "type": "function",
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.input_schema,
                    "strict": false,
                })
            })
            .collect();

        payload
            .as_object_mut()
            .unwrap()
            .insert("tools".to_string(), json!(tools_spec));
    }

    if !is_reasoning_model {
        if let Some(temp) = model_config.temperature {
            payload
                .as_object_mut()
                .unwrap()
                .insert("temperature".to_string(), json!(temp));
        }
    }

    if let Some(max_tokens) = model_config.max_tokens {
        payload
            .as_object_mut()
            .unwrap()
            .insert("max_output_tokens".to_string(), json!(max_tokens));
    }

    Ok(payload)
}

fn sanitize_tool_arguments(value: Value) -> anyhow::Result<Value> {
    match value {
        Value::String(text) => Ok(Value::String(strip_unicode_tags(&text))),
        Value::Array(values) => Ok(Value::Array(
            values
                .into_iter()
                .map(sanitize_tool_arguments)
                .collect::<anyhow::Result<_>>()?,
        )),
        Value::Object(values) => {
            let mut sanitized = serde_json::Map::new();
            for (key, value) in values {
                let key = strip_unicode_tags(&key);
                if sanitized.contains_key(&key) {
                    return Err(anyhow!(
                        "Responses tool arguments contain duplicate key after Unicode tag sanitization"
                    ));
                }
                sanitized.insert(key, sanitize_tool_arguments(value)?);
            }
            Ok(Value::Object(sanitized))
        }
        value => Ok(value),
    }
}

fn parse_tool_arguments(arguments: &str) -> anyhow::Result<Value> {
    if arguments.is_empty() {
        Ok(json!({}))
    } else {
        match serde_json::from_str(arguments) {
            Ok(value) => sanitize_tool_arguments(value),
            Err(_) => Ok(json!({})),
        }
    }
}

fn sanitize_tool_request_id(id: &str, seen_ids: &mut HashSet<String>) -> anyhow::Result<String> {
    let id = strip_unicode_tags(id);
    if !seen_ids.insert(id.clone()) {
        return Err(anyhow!(
            "Responses tool calls contain duplicate ID after Unicode tag sanitization"
        ));
    }
    Ok(id)
}

pub fn responses_api_to_message(response: &ResponsesApiResponse) -> anyhow::Result<Message> {
    let mut content = Vec::new();
    let mut tool_request_ids = HashSet::new();

    for item in &response.output {
        match item {
            ResponseOutputItem::Reasoning { summary, .. } => {
                content.extend(reasoning_from_summary(summary));
            }
            ResponseOutputItem::Message {
                content: msg_content,
                ..
            } => {
                for block in msg_content {
                    match block {
                        ResponseContentBlock::OutputText { text, .. } => {
                            let text = sanitize_unicode_tags(text);
                            if !text.is_empty() {
                                content.push(MessageContentBlock::text(text));
                            }
                        }
                        ResponseContentBlock::Refusal { refusal } => {
                            let refusal = sanitize_unicode_tags(refusal);
                            if !refusal.is_empty() {
                                content.push(MessageContentBlock::text(refusal));
                            }
                        }
                        ResponseContentBlock::ReasoningText { text } => {
                            let text = sanitize_unicode_tags(text);
                            if !text.is_empty() {
                                content.push(MessageContentBlock::thinking(text, ""));
                            }
                        }
                        ResponseContentBlock::ToolCall { id, name, input } => {
                            let id = sanitize_tool_request_id(id, &mut tool_request_ids)?;
                            content.push(MessageContentBlock::tool_request(
                                id,
                                Ok(CallToolRequestParams::new(strip_unicode_tags(name))
                                    .with_arguments(object(sanitize_tool_arguments(
                                        input.clone(),
                                    )?))),
                            ));
                        }
                    }
                }
            }
            ResponseOutputItem::FunctionCall {
                id,
                call_id,
                name,
                arguments,
                ..
            } => {
                let request_id = call_id.clone().or_else(|| id.clone()).ok_or_else(|| {
                    anyhow!("Responses function_call output missing call_id and id")
                })?;
                let request_id = sanitize_tool_request_id(&request_id, &mut tool_request_ids)?;
                let parsed_args = parse_tool_arguments(arguments)?;

                content.push(MessageContentBlock::tool_request(
                    request_id,
                    Ok(CallToolRequestParams::new(strip_unicode_tags(name))
                        .with_arguments(object(parsed_args))),
                ));
            }
        }
    }

    let mut message = Message::new(Role::Assistant, chrono::Utc::now().timestamp(), content);

    message = message.with_id(response.id.clone());

    Ok(message)
}

pub fn get_responses_usage(response: &ResponsesApiResponse) -> Usage {
    response
        .usage
        .as_ref()
        .map_or_else(Usage::default, ResponseUsage::to_usage)
}

fn process_streaming_output_items(
    output_items: Vec<ResponseOutputItemInfo>,
    is_text_response: bool,
) -> anyhow::Result<Vec<MessageContentBlock>> {
    let mut content = Vec::new();
    let mut tool_request_ids = HashSet::new();

    for item in output_items {
        match item {
            ResponseOutputItemInfo::Reasoning { summary, .. } => {
                content.extend(reasoning_from_summary(&summary));
            }
            ResponseOutputItemInfo::Message { content: parts, .. } => {
                for part in parts {
                    match part {
                        ContentBlockPart::OutputText { text, .. } => {
                            let text = sanitize_unicode_tags(&text);
                            if !text.is_empty() && !is_text_response {
                                content.push(MessageContentBlock::text(text));
                            }
                        }
                        ContentBlockPart::Refusal { refusal } => {
                            let refusal = sanitize_unicode_tags(&refusal);
                            if !refusal.is_empty() && !is_text_response {
                                content.push(MessageContentBlock::text(refusal));
                            }
                        }
                        ContentBlockPart::ReasoningText { text } => {
                            let text = sanitize_unicode_tags(&text);
                            if !text.is_empty() {
                                content.push(MessageContentBlock::thinking(text, ""));
                            }
                        }
                        ContentBlockPart::ToolCall {
                            id,
                            name,
                            arguments,
                        } => {
                            let id = sanitize_tool_request_id(&id, &mut tool_request_ids)?;
                            let parsed_args = parse_tool_arguments(&arguments)?;

                            content.push(MessageContentBlock::tool_request(
                                id,
                                Ok(CallToolRequestParams::new(strip_unicode_tags(&name))
                                    .with_arguments(object(parsed_args))),
                            ));
                        }
                    }
                }
            }
            ResponseOutputItemInfo::FunctionCall {
                id,
                call_id,
                name,
                arguments,
                ..
            } => {
                let request_id = call_id.or(id).ok_or_else(|| {
                    anyhow!("Responses function_call output missing call_id and id")
                })?;
                let request_id = sanitize_tool_request_id(&request_id, &mut tool_request_ids)?;
                let parsed_args = parse_tool_arguments(&arguments)?;

                content.push(MessageContentBlock::tool_request(
                    request_id,
                    Ok(CallToolRequestParams::new(strip_unicode_tags(&name))
                        .with_arguments(object(parsed_args))),
                ));
            }
        }
    }

    Ok(content)
}

fn output_token_limit_marker(id: Option<String>) -> Message {
    let mut message = Message::assistant();
    if let Some(id) = id {
        message = message.with_id(id);
    }
    message.metadata.output_token_limit_reached = true;
    message
}

/// Parse a line per the SSE grammar and return its field name:
/// - `field: value` / `field:value` -> `Some(field)`
/// - `field` (no colon, empty value) -> `Some(field)`
/// - `: comment` -> `Some("")`
///
/// Returns `None` when the line does not look like an SSE field (e.g. a
/// bare JSON payload such as `{"type": ...}`), so callers can fall back
/// to parsing it as JSON.
fn sse_field_name(line: &str) -> Option<&str> {
    let field = line.split_once(':').map_or(line, |(name, _)| name);
    if field.is_empty() {
        return Some("");
    }
    let field_like = !field.contains(char::is_whitespace)
        && field
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'));
    field_like.then_some(field)
}

pub fn responses_api_to_streaming_message<S>(
    mut stream: S,
) -> impl Stream<Item = anyhow::Result<(Option<Message>, Option<ProviderUsage>)>> + 'static
where
    S: Stream<Item = anyhow::Result<String>> + Unpin + Send + 'static,
{
    try_stream! {
        use futures::StreamExt;

        let mut accumulated_text = String::new();
        let mut response_id: Option<String> = None;
        let mut model_name: Option<String> = None;
        let mut final_usage: Option<ProviderUsage> = None;
        let mut output_items: Vec<ResponseOutputItemInfo> = Vec::new();
        let mut is_text_response = false;
        let mut output_token_limit_reached = false;

        'outer: while let Some(response) = stream.next().await {
            let response_str = response?;

            // Skip empty lines
            if response_str.trim().is_empty() {
                continue;
            }
            if response_str.starts_with(':') {
                continue;
            }

            // Parse SSE format: "event: <type>\ndata: <json>"
            // For now, we only care about the data line
            // SSE spec allows both "data: value" and "data:value" (space after colon is optional)
            let data_line = if response_str.starts_with("data: ") {
                response_str.strip_prefix("data: ").unwrap()
            } else if response_str.starts_with("data:") {
                response_str.strip_prefix("data:").unwrap()
            } else if sse_field_name(&response_str).is_some_and(|f| f != "data") {
                // Skip payload-free SSE fields: event, id, retry, comments,
                // colon-less fields with empty values, and unknown extension
                // fields — the SSE spec requires all of these to be ignored.
                continue;
            } else {
                // Try to parse as-is when there's no prefix (bare JSON frames)
                &response_str
            };

            if data_line == "[DONE]" {
                break 'outer;
            }

            let Some(event) = parse_responses_stream_event(data_line)? else {
                continue;
            };

            match event {
                ResponsesStreamEvent::ResponseCreated { response, .. } |
                ResponsesStreamEvent::ResponseInProgress { response, .. } => {
                    response_id = Some(response.id);
                    model_name = Some(response.model);
                }

                ResponsesStreamEvent::OutputTextDelta { delta, .. } => {
                    is_text_response = true;
                    let delta = strip_unicode_tags(&delta);
                    if !delta.is_empty() {
                        accumulated_text.push_str(&delta);

                        // Yield incremental text updates for true streaming
                        let mut msg = Message::new(
                            Role::Assistant,
                            chrono::Utc::now().timestamp(),
                            vec![MessageContentBlock::text(&delta)],
                        );

                        // Add ID so desktop client knows these deltas are part of the same message
                        if let Some(id) = &response_id {
                            msg = msg.with_id(id.clone());
                        }

                        yield (Some(msg), None);
                    }
                }

                ResponsesStreamEvent::OutputItemDone { item, .. } => {
                    output_items.push(item);
                }

                ResponsesStreamEvent::OutputTextDone { .. } => {
                    // Text is already complete from deltas, this is just a summary event
                }

                ResponsesStreamEvent::ResponseCompleted { response, .. } => {
                    let model = model_name.as_ref().unwrap_or(&response.model);
                    let usage = response.usage.as_ref().map_or_else(
                        Usage::default,
                        ResponseUsage::to_usage,
                    );
                    let mut pu = ProviderUsage::new(model.clone(), usage);
                    pu.finish_reasons = Some(vec![response.status.clone()]);
                    pu.response_id = Some(response.id.clone());
                    final_usage = Some(pu);

                    // For complete output, use the response output items
                    if !response.output.is_empty() {
                        output_items = response.output;
                    }

                    break 'outer;
                }

                ResponsesStreamEvent::ResponseIncomplete { response, .. } => {
                    let model = model_name.as_ref().unwrap_or(&response.model);
                    let usage = response.usage.as_ref().map_or_else(
                        Usage::default,
                        ResponseUsage::to_usage,
                    );
                    let mut pu = ProviderUsage::new(model.clone(), usage);
                    pu.finish_reasons = Some(vec![response
                        .incomplete_details
                        .as_ref()
                        .and_then(|details| details.reason.clone())
                        .unwrap_or_else(|| response.status.clone())]);
                    pu.response_id = Some(response.id.clone());
                    final_usage = Some(pu);
                    response_id = Some(response.id.clone());
                    output_token_limit_reached = response_reached_output_token_limit(
                        &response.status,
                        response.incomplete_details.as_ref(),
                    );

                    if !response.output.is_empty() {
                        output_items = response
                            .output
                            .into_iter()
                            .filter(|item| match item {
                                ResponseOutputItemInfo::FunctionCall { status, .. } => {
                                    status.as_deref() == Some("completed")
                                }
                                _ => true,
                            })
                            .collect();
                    }

                    break 'outer;
                }

                ResponsesStreamEvent::FunctionCallArgumentsDelta { .. } => {
                    // Function call arguments are being streamed, but we'll get the complete
                    // arguments in the OutputItemDone event, so we can ignore deltas for now
                }

                ResponsesStreamEvent::FunctionCallArgumentsDone { .. } => {
                    // Arguments are complete, will be in the OutputItemDone event
                }

                ResponsesStreamEvent::RefusalDelta { delta, .. } => {
                    is_text_response = true;
                    let delta = strip_unicode_tags(&delta);
                    if !delta.is_empty() {
                        accumulated_text.push_str(&delta);

                        let mut msg = Message::new(
                            Role::Assistant,
                            chrono::Utc::now().timestamp(),
                            vec![MessageContentBlock::text(&delta)],
                        );

                        if let Some(id) = &response_id {
                            msg = msg.with_id(id.clone());
                        }

                        yield (Some(msg), None);
                    }
                }

                ResponsesStreamEvent::RefusalDone { .. } => {
                    // Refusal text already streamed via deltas
                }

                ResponsesStreamEvent::ResponseFailed { error, .. } => {
                    Err::<(), ProviderError>(ProviderError::RequestFailed(format!(
                        "Responses API failed: {:?}",
                        error
                    )))?;
                }

                ResponsesStreamEvent::Error { error } => {
                    Err::<(), ProviderError>(ProviderError::RequestFailed(format!(
                        "Responses API error: {:?}",
                        error
                    )))?;
                }

                _ => {
                    // Ignore other event types (OutputItemAdded, ContentBlockPartAdded, ContentBlockPartDone)
                }
            }
        }

        // Process final output items and yield usage data
        let content = process_streaming_output_items(output_items, is_text_response)?;

        if !content.is_empty() {
            let mut message = Message::new(Role::Assistant, chrono::Utc::now().timestamp(), content);
            if let Some(id) = response_id {
                message = message.with_id(id);
            }
            message.metadata.output_token_limit_reached = output_token_limit_reached;
            yield (Some(message), final_usage);
        } else if output_token_limit_reached {
            yield (Some(output_token_limit_marker(response_id)), final_usage);
        } else if let Some(usage) = final_usage {
            yield (None, Some(usage));
        }
    }
}

#[cfg(test)]
mod document_tests {
    use super::*;

    fn format(messages: &[Message]) -> Vec<Value> {
        let mut items = Vec::new();
        add_message_items(&mut items, messages, true);
        items
    }

    #[test]
    fn user_document_becomes_an_input_file_item() {
        let items = format(&[Message::user().with_document(
            "cGRmLWJ5dGVz",
            "application/pdf",
            Some("q3-report.pdf".to_string()),
        )]);

        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["role"], "user");
        assert_eq!(
            items[0]["content"][0],
            json!({
                "type": "input_file",
                "filename": "q3-report.pdf",
                "file_data": "data:application/pdf;base64,cGRmLWJ5dGVz",
            })
        );
    }

    #[test]
    fn assistant_document_becomes_an_output_text_item() {
        let items = format(&[Message::assistant().with_document(
            "cGRmLWJ5dGVz",
            "application/pdf",
            Some("q3-report.pdf".to_string()),
        )]);

        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["role"], "assistant");
        let part = &items[0]["content"][0];
        assert_eq!(part["type"], "output_text");
        let text = part["text"].as_str().unwrap();
        assert!(text.contains("q3-report.pdf"), "{text}");
        assert!(text.contains("user messages"), "{text}");
        assert!(!text.contains("cGRmLWJ5dGVz"), "{text}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::message::MessageContentBlock;
    use crate::model::ModelConfig;
    use futures::StreamExt;
    use rmcp::model::CallToolRequestParams;
    use rmcp::object;

    #[tokio::test]
    async fn test_responses_stream_ignores_keepalive_event() -> anyhow::Result<()> {
        let lines = vec![
            r#"data: {"type":"response.created","sequence_number":1,"response":{"id":"resp_1","object":"response","created_at":1737368310,"status":"in_progress","model":"gpt-5.2-pro","output":[]}}"#.to_string(),
            r#"data: {"type":"keepalive"}"#.to_string(),
            r#"data: {"type":"response.output_text.delta","sequence_number":2,"item_id":"msg_1","output_index":0,"content_index":0,"delta":"Hello"}"#.to_string(),
            r#"data: {"type":"response.output_text.delta","sequence_number":3,"item_id":"msg_1","output_index":0,"content_index":0,"delta":" world"}"#.to_string(),
            r#"data: {"type":"response.completed","sequence_number":4,"response":{"id":"resp_1","object":"response","created_at":1737368310,"status":"completed","model":"gpt-5.2-pro","output":[],"usage":{"input_tokens":10,"output_tokens":4,"total_tokens":14,"input_tokens_details":{"cached_tokens":6}}}}"#.to_string(),
            "data: [DONE]".to_string(),
        ];

        let response_stream = tokio_stream::iter(lines.into_iter().map(Ok));
        let messages = responses_api_to_streaming_message(response_stream);
        futures::pin_mut!(messages);

        let mut text_parts = Vec::new();
        let mut usage: Option<ProviderUsage> = None;

        while let Some(item) = messages.next().await {
            let (message, maybe_usage) = item?;
            if let Some(msg) = message {
                for content in msg.content {
                    if let MessageContentBlock::Text(text) = content {
                        text_parts.push(text.text.clone());
                    }
                }
            }
            if let Some(final_usage) = maybe_usage {
                usage = Some(final_usage);
            }
        }

        assert_eq!(text_parts.concat(), "Hello world");
        let usage = usage.expect("usage should be present at completion");
        assert_eq!(usage.model, "gpt-5.2-pro");
        assert_eq!(usage.usage.input_tokens, Some(10));
        assert_eq!(usage.usage.output_tokens, Some(4));
        assert_eq!(usage.usage.total_tokens, Some(14));
        assert_eq!(usage.usage.cache_read_input_tokens, Some(6));
        assert_eq!(usage.usage.cache_write_input_tokens, None);

        Ok(())
    }

    #[tokio::test]
    async fn test_responses_stream_ignores_sse_field_lines() -> anyhow::Result<()> {
        let lines = vec![
            "id:1".to_string(),
            "id".to_string(),
            "x-trace: abc123".to_string(),
            "retry: 100".to_string(),
            ": keepalive comment".to_string(),
            r#"data: {"type":"response.created","sequence_number":1,"response":{"id":"resp_1","object":"response","created_at":1737368310,"status":"in_progress","model":"qwen3.8-max","output":[]}}"#.to_string(),
            "event: response.output_text.delta".to_string(),
            r#"data: {"type":"response.output_text.delta","sequence_number":2,"item_id":"msg_1","output_index":0,"content_index":0,"delta":"Hello"}"#.to_string(),
            r#"data: {"type":"response.output_text.delta","sequence_number":3,"item_id":"msg_1","output_index":0,"content_index":0,"delta":" world"}"#.to_string(),
            r#"data: {"type":"response.completed","sequence_number":4,"response":{"id":"resp_1","object":"response","created_at":1737368310,"status":"completed","model":"qwen3.8-max","output":[],"usage":{"input_tokens":10,"output_tokens":4,"total_tokens":14}}}"#.to_string(),
            "data: [DONE]".to_string(),
        ];

        let response_stream = tokio_stream::iter(lines.into_iter().map(Ok));
        let messages = responses_api_to_streaming_message(response_stream);
        futures::pin_mut!(messages);

        let mut text_parts = Vec::new();
        while let Some(item) = messages.next().await {
            let (message, _) = item?;
            if let Some(msg) = message {
                for content in msg.content {
                    if let MessageContentBlock::Text(text) = content {
                        text_parts.push(text.text.clone());
                    }
                }
            }
        }

        assert_eq!(text_parts.concat(), "Hello world");
        Ok(())
    }

    #[tokio::test]
    async fn test_responses_stream_completed_allows_missing_output() -> anyhow::Result<()> {
        let lines = vec![
            r#"data: {"type":"response.created","sequence_number":1,"response":{"id":"resp_1","object":"response","created_at":1737368310,"status":"in_progress","model":"gpt-5.2-pro","output":[]}}"#.to_string(),
            r#"data: {"type":"response.output_text.delta","sequence_number":2,"item_id":"msg_1","output_index":0,"content_index":0,"delta":"Hello"}"#.to_string(),
            r#"data: {"type":"response.output_text.delta","sequence_number":3,"item_id":"msg_1","output_index":0,"content_index":0,"delta":" world"}"#.to_string(),
            r#"data: {"type":"response.completed","sequence_number":4,"response":{"id":"resp_1","object":"response","created_at":1737368310,"status":"completed","model":"gpt-5.2-pro","usage":{"input_tokens":10,"output_tokens":4,"total_tokens":14}}}"#.to_string(),
            "data: [DONE]".to_string(),
        ];

        let response_stream = tokio_stream::iter(lines.into_iter().map(Ok));
        let messages = responses_api_to_streaming_message(response_stream);
        futures::pin_mut!(messages);

        let mut text_parts = Vec::new();
        let mut usage: Option<ProviderUsage> = None;

        while let Some(item) = messages.next().await {
            let (message, maybe_usage) = item?;
            if let Some(msg) = message {
                for content in msg.content {
                    if let MessageContentBlock::Text(text) = content {
                        text_parts.push(text.text.clone());
                    }
                }
            }
            if let Some(final_usage) = maybe_usage {
                usage = Some(final_usage);
            }
        }

        assert_eq!(text_parts.concat(), "Hello world");
        let usage = usage.expect("usage should be present at completion");
        assert_eq!(usage.model, "gpt-5.2-pro");
        assert_eq!(usage.usage.input_tokens, Some(10));
        assert_eq!(usage.usage.output_tokens, Some(4));
        assert_eq!(usage.usage.total_tokens, Some(14));

        Ok(())
    }

    #[tokio::test]
    async fn test_responses_stream_marks_output_token_limit_when_incomplete() -> anyhow::Result<()>
    {
        let lines = vec![
            r#"data: {"type":"response.created","sequence_number":1,"response":{"id":"resp_1","object":"response","created_at":1737368310,"status":"in_progress","model":"gpt-5.2-pro","output":[]}}"#.to_string(),
            r#"data: {"type":"response.output_text.delta","sequence_number":2,"item_id":"msg_1","output_index":0,"content_index":0,"delta":"Partial response"}"#.to_string(),
            r#"data: {"type":"response.incomplete","sequence_number":3,"response":{"id":"resp_1","object":"response","created_at":1737368310,"status":"incomplete","model":"gpt-5.2-pro","output":[],"incomplete_details":{"reason":"max_output_tokens"},"usage":{"input_tokens":10,"output_tokens":5,"total_tokens":15}}}"#.to_string(),
            "data: [DONE]".to_string(),
        ];

        let response_stream = tokio_stream::iter(lines.into_iter().map(Ok));
        let messages = responses_api_to_streaming_message(response_stream);
        futures::pin_mut!(messages);

        let mut text_parts = Vec::new();
        let mut output_token_limit_markers = Vec::new();
        let mut usage: Option<ProviderUsage> = None;

        while let Some(item) = messages.next().await {
            let (message, maybe_usage) = item?;
            if let Some(msg) = message {
                if msg.metadata.output_token_limit_reached {
                    output_token_limit_markers.push((msg.id.clone(), msg.content.is_empty()));
                }
                for content in msg.content {
                    if let MessageContentBlock::Text(text) = content {
                        text_parts.push(text.text);
                    }
                }
            }
            if let Some(final_usage) = maybe_usage {
                usage = Some(final_usage);
            }
        }

        assert_eq!(text_parts.concat(), "Partial response");
        assert_eq!(
            output_token_limit_markers,
            vec![(Some("resp_1".to_string()), true)]
        );
        let usage = usage.expect("usage should be present when the response is incomplete");
        assert_eq!(usage.model, "gpt-5.2-pro");
        assert_eq!(usage.usage.input_tokens, Some(10));
        assert_eq!(usage.usage.output_tokens, Some(5));
        assert_eq!(usage.usage.total_tokens, Some(15));
        assert_eq!(
            usage.finish_reasons.as_deref(),
            Some(&["max_output_tokens".to_string()][..])
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_responses_stream_drops_incomplete_function_calls() -> anyhow::Result<()> {
        let lines = vec![
            r#"data: {"type":"response.created","sequence_number":1,"response":{"id":"resp_1","object":"response","created_at":1737368310,"status":"in_progress","model":"gpt-5.2-pro","output":[]}}"#.to_string(),
            r#"data: {"type":"response.incomplete","sequence_number":2,"response":{"id":"resp_1","object":"response","created_at":1737368310,"status":"incomplete","model":"gpt-5.2-pro","output":[{"type":"function_call","id":"fc_complete","status":"completed","call_id":"call_complete","name":"read_file","arguments":"{\"path\":\"README.md\"}"},{"type":"function_call","id":"fc_partial","status":"in_progress","call_id":"call_partial","name":"shell","arguments":"{\"command\":\"echo"}],"incomplete_details":{"reason":"max_output_tokens"},"usage":{"input_tokens":10,"output_tokens":5,"total_tokens":15}}}"#.to_string(),
            "data: [DONE]".to_string(),
        ];

        let response_stream = tokio_stream::iter(lines.into_iter().map(Ok));
        let messages = responses_api_to_streaming_message(response_stream);
        futures::pin_mut!(messages);

        let mut tool_request_ids = Vec::new();
        let mut output_token_limit_reached = false;
        let mut usage: Option<ProviderUsage> = None;

        while let Some(item) = messages.next().await {
            let (message, maybe_usage) = item?;
            if let Some(msg) = message {
                output_token_limit_reached |= msg.metadata.output_token_limit_reached;
                for content in msg.content {
                    if let MessageContentBlock::ToolRequest(request) = content {
                        tool_request_ids.push(request.id);
                    }
                }
            }
            if let Some(final_usage) = maybe_usage {
                usage = Some(final_usage);
            }
        }

        assert_eq!(tool_request_ids, vec!["call_complete"]);
        assert!(output_token_limit_reached);
        let usage = usage.expect("usage should be present when the response is incomplete");
        assert_eq!(usage.usage.input_tokens, Some(10));
        assert_eq!(usage.usage.output_tokens, Some(5));
        assert_eq!(usage.usage.total_tokens, Some(15));

        Ok(())
    }

    #[tokio::test]
    async fn test_responses_stream_allows_message_output_without_id_status() -> anyhow::Result<()> {
        let lines = vec![
            r#"data: {"type":"response.created","sequence_number":1,"response":{"id":"resp_1","object":"response","created_at":1737368310,"status":"in_progress","model":"gpt-5.2-pro","output":[]}}"#.to_string(),
            r#"data: {"type":"response.output_text.delta","sequence_number":2,"item_id":"msg_1","output_index":0,"content_index":0,"delta":"Hello"}"#.to_string(),
            r#"data: {"type":"response.output_text.delta","sequence_number":3,"item_id":"msg_1","output_index":0,"content_index":0,"delta":" world"}"#.to_string(),
            r#"data: {"type":"response.completed","sequence_number":4,"response":{"id":"resp_1","object":"response","created_at":1737368310,"status":"completed","model":"gpt-5.2-pro","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Hello world"}]}],"usage":{"input_tokens":10,"output_tokens":4,"total_tokens":14}}}"#.to_string(),
            "data: [DONE]".to_string(),
        ];

        let response_stream = tokio_stream::iter(lines.into_iter().map(Ok));
        let messages = responses_api_to_streaming_message(response_stream);
        futures::pin_mut!(messages);

        let mut text_parts = Vec::new();
        let mut usage: Option<ProviderUsage> = None;

        while let Some(item) = messages.next().await {
            let (message, maybe_usage) = item?;
            if let Some(msg) = message {
                for content in msg.content {
                    if let MessageContentBlock::Text(text) = content {
                        text_parts.push(text.text.clone());
                    }
                }
            }
            if let Some(final_usage) = maybe_usage {
                usage = Some(final_usage);
            }
        }

        assert_eq!(text_parts.concat(), "Hello world");
        let usage = usage.expect("usage should be present at completion");
        assert_eq!(usage.model, "gpt-5.2-pro");
        assert_eq!(usage.usage.input_tokens, Some(10));
        assert_eq!(usage.usage.output_tokens, Some(4));
        assert_eq!(usage.usage.total_tokens, Some(14));

        Ok(())
    }

    #[tokio::test]
    async fn test_responses_stream_allows_function_call_without_id_status() -> anyhow::Result<()> {
        let lines = vec![
            r#"data: {"type":"response.created","sequence_number":1,"response":{"id":"resp_1","object":"response","created_at":1737368310,"status":"in_progress","model":"gpt-5.2-pro","output":[]}}"#.to_string(),
            r#"data: {"type":"response.completed","sequence_number":2,"response":{"id":"resp_1","object":"response","created_at":1737368310,"status":"completed","model":"gpt-5.2-pro","output":[{"type":"reasoning","summary":[]},{"type":"function_call","call_id":"call_abc","name":"shell","arguments":"{\"command\":\"pwd\"}"}],"usage":{"input_tokens":10,"output_tokens":4,"total_tokens":14}}}"#.to_string(),
            "data: [DONE]".to_string(),
        ];

        let response_stream = tokio_stream::iter(lines.into_iter().map(Ok));
        let messages = responses_api_to_streaming_message(response_stream);
        futures::pin_mut!(messages);

        let mut tool_request_id = None;
        let mut usage: Option<ProviderUsage> = None;

        while let Some(item) = messages.next().await {
            let (message, maybe_usage) = item?;
            if let Some(msg) = message {
                for content in msg.content {
                    if let MessageContentBlock::ToolRequest(request) = content {
                        tool_request_id = Some(request.id);
                    }
                }
            }
            if let Some(final_usage) = maybe_usage {
                usage = Some(final_usage);
            }
        }

        assert_eq!(tool_request_id.as_deref(), Some("call_abc"));
        let usage = usage.expect("usage should be present at completion");
        assert_eq!(usage.model, "gpt-5.2-pro");
        assert_eq!(usage.usage.total_tokens, Some(14));

        Ok(())
    }

    #[test]
    fn test_responses_api_to_message_captures_reasoning_summary() -> anyhow::Result<()> {
        let response: ResponsesApiResponse = serde_json::from_value(serde_json::json!({
            "id": "resp_1",
            "object": "response",
            "created_at": 1737368310,
            "status": "completed",
            "model": "gpt-5",
            "output": [
                {
                    "type": "reasoning",
                    "id": "rs_1",
                    "summary": [
                        { "type": "summary_text", "text": "Thinking\u{E0041} about the question..." },
                        { "type": "summary_text", "text": "The answer is\u{E0042} straightforward." }
                    ]
                },
                {
                    "type": "message",
                    "id": "msg_1",
                    "status": "completed",
                    "role": "assistant",
                    "content": [
                        { "type": "output_text", "text": "The capital of France is Paris." }
                    ]
                }
            ]
        }))?;

        let message = responses_api_to_message(&response)?;

        let thinking = message.content.iter().find_map(|c| c.as_thinking());
        assert!(thinking.is_some(), "should contain thinking content");
        assert_eq!(
            thinking.unwrap().thinking,
            "Thinking about the question...\nThe answer is straightforward."
        );

        let text = message.content.iter().find_map(|c| c.as_text());
        assert_eq!(text, Some("The capital of France is Paris."));

        Ok(())
    }

    #[tokio::test]
    async fn test_responses_stream_captures_reasoning_summary() -> anyhow::Result<()> {
        let reasoning_item = serde_json::json!({
            "type": "reasoning",
            "id": "rs_1",
            "summary": [
                { "type": "summary_text", "text": "Let me\u{E0041} think step by step." }
            ]
        });
        let message_item = serde_json::json!({
            "type": "message",
            "id": "msg_1",
            "status": "completed",
            "role": "assistant",
            "content": [{ "type": "output_text", "text": "Paris." }]
        });

        let lines = vec![
            format!(
                r#"data: {{"type":"response.created","sequence_number":1,"response":{{"id":"resp_1","object":"response","created_at":1737368310,"status":"in_progress","model":"gpt-5","output":[]}}}}"#
            ),
            format!(
                r#"data: {{"type":"response.output_text.delta","sequence_number":2,"item_id":"msg_1","output_index":1,"content_index":0,"delta":"Paris."}}"#
            ),
            format!(
                r#"data: {{"type":"response.output_item.done","sequence_number":3,"output_index":0,"item":{}}}"#,
                serde_json::to_string(&reasoning_item)?
            ),
            format!(
                r#"data: {{"type":"response.output_item.done","sequence_number":4,"output_index":1,"item":{}}}"#,
                serde_json::to_string(&message_item)?
            ),
            format!(
                r#"data: {{"type":"response.completed","sequence_number":5,"response":{{"id":"resp_1","object":"response","created_at":1737368310,"status":"completed","model":"gpt-5","output":[{},{}],"usage":{{"input_tokens":10,"output_tokens":5,"total_tokens":15}}}}}}"#,
                serde_json::to_string(&reasoning_item)?,
                serde_json::to_string(&message_item)?
            ),
            "data: [DONE]".to_string(),
        ];

        let response_stream = tokio_stream::iter(lines.into_iter().map(Ok));
        let messages = responses_api_to_streaming_message(response_stream);
        futures::pin_mut!(messages);

        let mut thinking_parts = Vec::new();
        let mut text_parts = Vec::new();

        while let Some(item) = messages.next().await {
            let (message, _) = item?;
            if let Some(msg) = message {
                for content in msg.content {
                    match &content {
                        MessageContentBlock::Thinking(t) => thinking_parts.push(t.thinking.clone()),
                        MessageContentBlock::Text(t) => text_parts.push(t.text.clone()),
                        _ => {}
                    }
                }
            }
        }

        assert!(
            !thinking_parts.is_empty(),
            "should capture thinking from stream"
        );
        assert_eq!(thinking_parts.join(""), "Let me think step by step.");
        assert!(text_parts.concat().contains("Paris."));

        Ok(())
    }

    #[tokio::test]
    async fn test_responses_stream_error_event_still_returns_error() -> anyhow::Result<()> {
        let lines = vec![
            r#"data: {"type":"error","error":{"message":"boom"}}"#.to_string(),
            "data: [DONE]".to_string(),
        ];

        let response_stream = tokio_stream::iter(lines.into_iter().map(Ok));
        let messages = responses_api_to_streaming_message(response_stream);
        futures::pin_mut!(messages);

        let first = messages
            .next()
            .await
            .expect("stream should emit an error item");

        assert!(first.is_err());
        assert!(first
            .expect_err("expected error")
            .to_string()
            .contains("Responses API error"));

        Ok(())
    }

    #[test]
    fn test_history_preserves_chronological_order() {
        let model_config = ModelConfig {
            model_name: "gpt-5.2-codex".to_string(),
            context_limit: None,
            temperature: None,
            max_tokens: None,
            toolshim: false,
            toolshim_model: None,
            request_params: None,
            reasoning: None,
            supports_vision: None,
            request_headers: None,
        };

        let messages = vec![
            Message::assistant()
                .with_text("I'll create that file.")
                .with_tool_request(
                    "call_1",
                    Ok(CallToolRequestParams::new("shell")
                        .with_arguments(object!({"command": "echo hello"}))),
                ),
            Message::assistant()
                .with_text("Now let me verify.")
                .with_tool_request(
                    "call_2",
                    Ok(CallToolRequestParams::new("shell")
                        .with_arguments(object!({"command": "cat file.txt"}))),
                ),
        ];

        let result = create_responses_request(&model_config, "", &messages, &[]).unwrap();
        let input = result["input"].as_array().unwrap();

        let types: Vec<&str> = input
            .iter()
            .map(|item| item["type"].as_str().unwrap())
            .collect();

        assert_eq!(
            types,
            vec!["message", "function_call", "message", "function_call"]
        );
    }

    #[test]
    fn test_responses_api_to_message_uses_call_id_for_tool_request_id() {
        let response = ResponsesApiResponse {
            id: "resp_1".to_string(),
            object: "response".to_string(),
            created_at: 0,
            status: "completed".to_string(),
            model: "gpt-5.3-codex".to_string(),
            output: vec![ResponseOutputItem::FunctionCall {
                id: Some("fc_123".to_string()),
                status: Some("completed".to_string()),
                call_id: Some("call_abc".to_string()),
                name: "test__get_person_zip_code".to_string(),
                arguments: r#"{"name":"Alice Burns"}"#.to_string(),
            }],
            reasoning: None,
            usage: None,
        };

        let message = responses_api_to_message(&response).unwrap();
        assert_eq!(message.content.len(), 1);
        let MessageContentBlock::ToolRequest(tool_request) = &message.content[0] else {
            panic!("expected tool request content");
        };
        assert_eq!(tool_request.id, "call_abc");
    }

    #[test]
    fn test_responses_api_to_message_sanitizes_tool_arguments() {
        let response = ResponsesApiResponse {
            id: "resp_1".to_string(),
            object: "response".to_string(),
            created_at: 0,
            status: "completed".to_string(),
            model: "gpt-5.3-codex".to_string(),
            output: vec![
                ResponseOutputItem::Message {
                    id: Some("msg_1".to_string()),
                    status: Some("completed".to_string()),
                    role: "assistant".to_string(),
                    content: vec![ResponseContentBlock::ToolCall {
                        id: "call_\u{E0041}1".to_string(),
                        name: "sh\u{E0041}ell".to_string(),
                        input: json!({"prompt": "visible e\u{301}\u{E0041}text"}),
                    }],
                },
                ResponseOutputItem::FunctionCall {
                    id: None,
                    status: Some("completed".to_string()),
                    call_id: Some("call_\u{E0042}2".to_string()),
                    name: "sh\u{E0042}ell".to_string(),
                    arguments: serde_json::to_string(
                        &json!({"prompt": "visible e\u{301}\u{E0042}text"}),
                    )
                    .unwrap(),
                },
            ],
            reasoning: None,
            usage: None,
        };

        let message = responses_api_to_message(&response).unwrap();
        for (content, expected_id) in message.content.into_iter().zip(["call_1", "call_2"]) {
            let MessageContentBlock::ToolRequest(tool_request) = content else {
                panic!("expected tool request content");
            };
            assert_eq!(tool_request.id, expected_id);
            let tool_call = tool_request.tool_call.expect("expected valid tool call");
            assert_eq!(tool_call.name, "shell");
            assert_eq!(
                tool_call
                    .arguments
                    .expect("expected arguments")
                    .get("prompt"),
                Some(&json!("visible e\u{301}text"))
            );
        }
    }

    #[test]
    fn test_responses_api_to_message_rejects_sanitized_tool_request_id_collisions() {
        let response = ResponsesApiResponse {
            id: "resp_1".to_string(),
            object: "response".to_string(),
            created_at: 0,
            status: "completed".to_string(),
            model: "gpt-5.3-codex".to_string(),
            output: vec![
                ResponseOutputItem::Message {
                    id: Some("msg_1".to_string()),
                    status: Some("completed".to_string()),
                    role: "assistant".to_string(),
                    content: vec![ResponseContentBlock::ToolCall {
                        id: "call_1".to_string(),
                        name: "shell".to_string(),
                        input: json!({}),
                    }],
                },
                ResponseOutputItem::FunctionCall {
                    id: None,
                    status: Some("completed".to_string()),
                    call_id: Some("call_\u{E0041}1".to_string()),
                    name: "shell".to_string(),
                    arguments: "{}".to_string(),
                },
            ],
            reasoning: None,
            usage: None,
        };

        let error = responses_api_to_message(&response).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("duplicate ID after Unicode tag sanitization"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn test_responses_api_to_message_rejects_sanitized_tool_argument_key_collisions() {
        let colliding_arguments = json!({
            "command": "visible",
            "comm\u{E0041}and": "hidden",
        });
        let responses = [
            ResponsesApiResponse {
                id: "resp_1".to_string(),
                object: "response".to_string(),
                created_at: 0,
                status: "completed".to_string(),
                model: "gpt-5.3-codex".to_string(),
                output: vec![ResponseOutputItem::Message {
                    id: Some("msg_1".to_string()),
                    status: Some("completed".to_string()),
                    role: "assistant".to_string(),
                    content: vec![ResponseContentBlock::ToolCall {
                        id: "call_1".to_string(),
                        name: "shell".to_string(),
                        input: colliding_arguments.clone(),
                    }],
                }],
                reasoning: None,
                usage: None,
            },
            ResponsesApiResponse {
                id: "resp_2".to_string(),
                object: "response".to_string(),
                created_at: 0,
                status: "completed".to_string(),
                model: "gpt-5.3-codex".to_string(),
                output: vec![ResponseOutputItem::FunctionCall {
                    id: None,
                    status: Some("completed".to_string()),
                    call_id: Some("call_2".to_string()),
                    name: "shell".to_string(),
                    arguments: serde_json::to_string(&colliding_arguments).unwrap(),
                }],
                reasoning: None,
                usage: None,
            },
        ];

        for response in responses {
            let error = responses_api_to_message(&response).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("duplicate key after Unicode tag sanitization"),
                "unexpected error: {error}"
            );
        }
    }

    #[test]
    fn test_deserialize_reasoning_info_with_null_effort() {
        let json = r#"{"effort": null}"#;
        let info: ResponseReasoningInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.effort, None);
        assert_eq!(info.summary, None);
    }

    #[test]
    fn test_deserialize_reasoning_info_with_effort() {
        let json = r#"{"effort": "high", "summary": "Thought deeply"}"#;
        let info: ResponseReasoningInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.effort.as_deref(), Some("high"));
        assert_eq!(info.summary.as_deref(), Some("Thought deeply"));
    }

    #[test]
    fn test_responses_tools_include_strict_false() {
        let model_config = ModelConfig {
            model_name: "gpt-5.4".to_string(),
            context_limit: None,
            temperature: None,
            max_tokens: None,
            toolshim: false,
            toolshim_model: None,
            request_params: None,
            reasoning: None,
            supports_vision: None,
            request_headers: None,
        };

        let tool = Tool::new(
            "shell",
            "Execute a shell command",
            object!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The command to run"
                    }
                },
                "required": ["command"]
            }),
        );

        let result =
            create_responses_request(&model_config, "You are helpful.", &[], &[tool]).unwrap();
        let tools = result["tools"]
            .as_array()
            .expect("tools should be an array");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["strict"], json!(false),
            "Responses API defaults strict to true, but MCP tool schemas are not strict-compatible; must explicitly set strict: false");
    }

    #[test]
    fn test_responses_request_with_explicit_effort_suffix() {
        for (model_name, expected_model, expected_effort) in [
            ("gpt-5.4-xhigh", "gpt-5.4", "xhigh"),
            ("databricks-gpt-5.4-high", "databricks-gpt-5.4", "high"),
            ("databricks-o3-none", "databricks-o3", "none"),
        ] {
            let model_config = ModelConfig {
                model_name: model_name.to_string(),
                context_limit: None,
                temperature: None,
                max_tokens: None,
                toolshim: false,
                toolshim_model: None,
                request_params: None,
                reasoning: None,
                supports_vision: None,
                request_headers: None,
            };

            let result =
                create_responses_request(&model_config, "You are helpful.", &[], &[]).unwrap();

            assert_eq!(
                result["model"], expected_model,
                "unexpected model for {model_name}"
            );
            assert_eq!(
                result["reasoning"]["effort"], expected_effort,
                "unexpected effort for {model_name}"
            );
            assert_eq!(result["reasoning"]["summary"], "auto");
        }
    }

    #[test]
    fn test_responses_request_with_normalized_effort_suffix() {
        let model_config = ModelConfig::new("o3-mini-high");

        let result = create_responses_request(&model_config, "You are helpful.", &[], &[]).unwrap();

        assert_eq!(result["model"], "o3-mini");
        assert_eq!(result["reasoning"]["effort"], "high");
        assert_eq!(result["reasoning"]["summary"], "auto");
    }

    #[test]
    fn test_responses_request_supports_gpt_5_6_xhigh_effort() {
        let model_config = ModelConfig::new("gpt-5.6-sol-xhigh");

        let result = create_responses_request(&model_config, "You are helpful.", &[], &[]).unwrap();

        assert_eq!(result["model"], "gpt-5.6-sol");
        assert_eq!(result["reasoning"]["effort"], "xhigh");
        assert_eq!(result["reasoning"]["summary"], "auto");
    }

    #[test]
    fn test_responses_request_gpt6_astra_off_uses_low_not_none() {
        for model_name in ["gpt-6-astra", "data_workflow_tools.goose.goose-gpt-6-astra"] {
            let model_config = ModelConfig::new(model_name)
                .with_thinking_effort(crate::thinking::ThinkingEffort::Off);

            let result =
                create_responses_request(&model_config, "You are helpful.", &[], &[]).unwrap();

            assert_eq!(result["model"], model_name, "{model_name}");
            assert_eq!(
                result["reasoning"]["effort"], "low",
                "{model_name} Off should serialize as low, not none"
            );
        }

        let model_config = ModelConfig::new("gpt-5.6-luna")
            .with_thinking_effort(crate::thinking::ThinkingEffort::Off);
        let result = create_responses_request(&model_config, "You are helpful.", &[], &[]).unwrap();
        assert_eq!(result["reasoning"]["effort"], "none");
    }

    #[test]
    fn test_responses_request_supports_gpt_5_6_reasoning_mode() {
        for model_name in [
            "gpt-5.6-sol",
            "openrouter/openai/gpt-5.6-sol",
            "vendor-gpt-5-6",
        ] {
            let model_config = ModelConfig::new(model_name).with_merged_request_params(
                std::collections::HashMap::from([("reasoning_mode".to_string(), json!("pro"))]),
            );

            let result =
                create_responses_request(&model_config, "You are helpful.", &[], &[]).unwrap();

            assert_eq!(result["reasoning"]["mode"], "pro");
            assert!(result["reasoning"].get("effort").is_none());
            assert!(result["reasoning"].get("summary").is_none());
        }
    }

    #[test]
    fn test_responses_request_rejects_reasoning_mode_for_non_gpt_5_6_model() {
        for model_name in ["gpt-5.5", "gpt-5.60", "notgpt-5.6", "gpt-5.6ish"] {
            let model_config = ModelConfig::new(model_name).with_merged_request_params(
                std::collections::HashMap::from([("reasoning_mode".to_string(), json!("pro"))]),
            );

            let error = create_responses_request(&model_config, "You are helpful.", &[], &[])
                .expect_err("reasoning mode should be gated to GPT-5.6 models");

            assert!(error
                .to_string()
                .contains("reasoning_mode is only supported for GPT-5.6 models"));
        }
    }

    #[test]
    fn test_responses_request_without_effort_suffix_omits_reasoning() {
        for model_name in ["gpt-5.4", "o3", "gpt-5-nano"] {
            let model_config = ModelConfig {
                model_name: model_name.to_string(),
                context_limit: None,
                temperature: None,
                max_tokens: None,
                toolshim: false,
                toolshim_model: None,
                request_params: None,
                reasoning: None,
                supports_vision: None,
                request_headers: None,
            };

            let result =
                create_responses_request(&model_config, "You are helpful.", &[], &[]).unwrap();

            assert_eq!(result["model"], model_name, "model should be unchanged");
            assert!(
                result.get("reasoning").is_none(),
                "reasoning should be omitted for {model_name} without explicit effort suffix"
            );
        }
    }

    #[test]
    fn test_responses_request_omits_default_max_output_tokens_for_unknown_model() {
        let model_config = ModelConfig::new("gpt-5.6-sol");

        let result = create_responses_request(&model_config, "You are helpful.", &[], &[]).unwrap();

        assert_eq!(result["model"], "gpt-5.6-sol");
        assert!(
            result.get("max_output_tokens").is_none(),
            "unknown/new models should not receive Goose's fallback max_output_tokens"
        );
    }

    #[test]
    fn test_responses_request_includes_canonical_max_output_tokens() {
        let model_config = ModelConfig::new("gpt-5.4").with_canonical_limits("openai");

        let result = create_responses_request(&model_config, "You are helpful.", &[], &[]).unwrap();

        assert_eq!(
            result["max_output_tokens"],
            model_config.max_tokens.unwrap()
        );
    }

    #[test]
    fn test_responses_request_non_reasoning_model_ignores_global_thinking_effort() {
        let _guard = env_lock::lock_env([("GOOSE_THINKING_EFFORT", Some("high"))]);
        let model_config = ModelConfig {
            model_name: "gpt-4o".to_string(),
            context_limit: None,
            temperature: None,
            max_tokens: None,
            toolshim: false,
            toolshim_model: None,
            request_params: None,
            reasoning: None,
            supports_vision: None,
            request_headers: None,
        };

        let result = create_responses_request(&model_config, "You are helpful.", &[], &[]).unwrap();

        assert_eq!(result["model"], "gpt-4o");
        assert!(
            result.get("reasoning").is_none(),
            "non-reasoning models should not receive reasoning config"
        );
    }

    #[test]
    fn test_request_params_override_store() {
        let model_config = ModelConfig {
            model_name: "o3".to_string(),
            context_limit: None,
            temperature: None,
            max_tokens: None,
            toolshim: false,
            toolshim_model: None,
            request_params: Some(std::collections::HashMap::from([(
                "store".to_string(),
                serde_json::json!(true),
            )])),
            reasoning: None,
            supports_vision: None,
            request_headers: None,
        };

        let result = create_responses_request(&model_config, "", &[], &[]).unwrap();

        assert_eq!(result["store"], true);
    }

    #[test]
    fn test_user_image_serialized_in_responses_request() {
        use crate::conversation::message::Message;

        let messages = vec![Message::user()
            .with_text("describe this image")
            .with_image("aW1hZ2VkYXRh", "image/png")];

        let model_config = ModelConfig {
            model_name: "gpt-5.5".to_string(),
            context_limit: None,
            temperature: None,
            max_tokens: None,
            toolshim: false,
            toolshim_model: None,
            request_params: None,
            reasoning: None,
            supports_vision: Some(true),
            request_headers: None,
        };

        let result =
            create_responses_request(&model_config, "You are helpful.", &messages, &[]).unwrap();

        let input = result["input"].as_array().unwrap();
        assert_eq!(input.len(), 2);

        assert_eq!(input[0]["role"], "system");

        assert_eq!(input[1]["role"], "user");
        let content = input[1]["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);

        assert_eq!(content[0]["type"], "input_text");
        assert_eq!(content[0]["text"], "describe this image");

        assert_eq!(content[1]["type"], "input_image");
        assert_eq!(
            content[1]["image_url"],
            "data:image/png;base64,aW1hZ2VkYXRh"
        );
    }

    #[test]
    fn test_tool_response_with_image_serializes_as_typed_array() {
        use crate::conversation::message::Message;
        use rmcp::model::{CallToolResult, ContentBlock};

        let messages = vec![
            Message::user().with_content(MessageContentBlock::tool_response(
                "call_1",
                Ok(CallToolResult::success(vec![
                    ContentBlock::text("caption"),
                    ContentBlock::image("a+/=".to_string(), "image/png".to_string()),
                ])),
            )),
        ];

        let model_config = ModelConfig {
            model_name: "gpt-5.5".to_string(),
            context_limit: None,
            temperature: None,
            max_tokens: None,
            toolshim: false,
            toolshim_model: None,
            request_params: None,
            reasoning: None,
            supports_vision: Some(true),
            request_headers: None,
        };

        let result = create_responses_request(&model_config, "", &messages, &[]).unwrap();
        let input = result["input"].as_array().unwrap();

        assert_eq!(input[0]["type"], "function_call_output");
        assert_eq!(input[0]["call_id"], "call_1");

        let output = input[0]["output"].as_array().unwrap();
        assert_eq!(output.len(), 2);
        assert_eq!(output[0], json!({"type": "input_text", "text": "caption"}));
        assert_eq!(
            output[1],
            json!({"type": "input_image", "image_url": "data:image/png;base64,a+/="})
        );
    }

    #[test]
    fn test_tool_request_serializes_function_call_with_arguments() {
        use crate::conversation::message::Message;

        let messages = vec![Message::assistant().with_tool_request(
            "call_1",
            Ok(CallToolRequestParams::new("search")
                .with_arguments(object!({"q": "rust", "limit": 2}))),
        )];

        let model_config = ModelConfig {
            model_name: "gpt-5.5".to_string(),
            context_limit: None,
            temperature: None,
            max_tokens: None,
            toolshim: false,
            toolshim_model: None,
            request_params: None,
            reasoning: None,
            supports_vision: None,
            request_headers: None,
        };

        let result = create_responses_request(&model_config, "", &messages, &[]).unwrap();
        let input = result["input"].as_array().unwrap();

        assert_eq!(input[0]["type"], "function_call");
        assert_eq!(input[0]["call_id"], "call_1");
        assert_eq!(input[0]["name"], "search");

        let args: serde_json::Value =
            serde_json::from_str(input[0]["arguments"].as_str().unwrap()).unwrap();
        assert_eq!(args["q"], "rust");
        assert_eq!(args["limit"], 2);
    }

    #[test]
    fn test_tool_request_none_arguments_serializes_empty_object() {
        use crate::conversation::message::Message;

        let messages = vec![Message::assistant()
            .with_tool_request("call_1", Ok(CallToolRequestParams::new("noop")))];

        let model_config = ModelConfig {
            model_name: "gpt-5.5".to_string(),
            context_limit: None,
            temperature: None,
            max_tokens: None,
            toolshim: false,
            toolshim_model: None,
            request_params: None,
            reasoning: None,
            supports_vision: None,
            request_headers: None,
        };

        let result = create_responses_request(&model_config, "", &messages, &[]).unwrap();
        let input = result["input"].as_array().unwrap();

        assert_eq!(input[0]["type"], "function_call");
        assert_eq!(input[0]["name"], "noop");
        assert_eq!(input[0]["arguments"], "{}");
    }

    #[test]
    fn test_text_flushed_before_tool_request() {
        use crate::conversation::message::Message;

        let messages = vec![Message::assistant()
            .with_text("planning")
            .with_tool_request(
                "call_1",
                Ok(CallToolRequestParams::new("shell").with_arguments(object!({"command": "ls"}))),
            )];

        let model_config = ModelConfig {
            model_name: "gpt-5.5".to_string(),
            context_limit: None,
            temperature: None,
            max_tokens: None,
            toolshim: false,
            toolshim_model: None,
            request_params: None,
            reasoning: None,
            supports_vision: None,
            request_headers: None,
        };

        let result = create_responses_request(&model_config, "", &messages, &[]).unwrap();
        let input = result["input"].as_array().unwrap();

        assert_eq!(input.len(), 2);
        assert_eq!(input[0]["role"], "assistant");
        assert_eq!(input[0]["content"][0]["type"], "output_text");
        assert_eq!(input[0]["content"][0]["text"], "planning");
        assert_eq!(input[1]["type"], "function_call");
    }

    #[test]
    fn test_text_flushed_before_tool_response() {
        use crate::conversation::message::Message;
        use rmcp::model::{CallToolResult, ContentBlock};

        let messages = vec![Message::user().with_text("context").with_content(
            MessageContentBlock::tool_response(
                "call_1",
                Ok(CallToolResult::success(vec![ContentBlock::text("done")])),
            ),
        )];

        let model_config = ModelConfig {
            model_name: "gpt-5.5".to_string(),
            context_limit: None,
            temperature: None,
            max_tokens: None,
            toolshim: false,
            toolshim_model: None,
            request_params: None,
            reasoning: None,
            supports_vision: None,
            request_headers: None,
        };

        let result = create_responses_request(&model_config, "", &messages, &[]).unwrap();
        let input = result["input"].as_array().unwrap();

        assert_eq!(input.len(), 2);
        assert_eq!(input[0]["role"], "user");
        assert_eq!(input[0]["content"][0]["type"], "input_text");
        assert_eq!(input[0]["content"][0]["text"], "context");
        assert_eq!(input[1]["type"], "function_call_output");
        assert_eq!(input[1]["output"], "done");
    }

    #[test]
    fn test_tool_response_error_serializes_with_error_prefix() {
        use crate::conversation::message::Message;
        use rmcp::model::{ErrorCode, ErrorData};

        let messages = vec![
            Message::user().with_content(MessageContentBlock::tool_response(
                "call_err",
                Err(ErrorData {
                    code: ErrorCode::INTERNAL_ERROR,
                    message: "file not found".into(),
                    data: None,
                }),
            )),
        ];

        let model_config = ModelConfig {
            model_name: "gpt-5.5".to_string(),
            context_limit: None,
            temperature: None,
            max_tokens: None,
            toolshim: false,
            toolshim_model: None,
            request_params: None,
            reasoning: None,
            supports_vision: None,
            request_headers: None,
        };

        let result = create_responses_request(&model_config, "", &messages, &[]).unwrap();
        let input = result["input"].as_array().unwrap();

        assert_eq!(input[0]["type"], "function_call_output");
        assert_eq!(input[0]["call_id"], "call_err");
        assert_eq!(input[0]["output"], "Error: file not found");
    }

    #[test]
    fn test_image_only_message_serializes() {
        use crate::conversation::message::Message;

        let messages = vec![Message::user().with_image("aW1n", "image/png")];

        let model_config = ModelConfig {
            model_name: "gpt-5.5".to_string(),
            context_limit: None,
            temperature: None,
            max_tokens: None,
            toolshim: false,
            toolshim_model: None,
            request_params: None,
            reasoning: None,
            supports_vision: Some(true),
            request_headers: None,
        };

        let result = create_responses_request(&model_config, "", &messages, &[]).unwrap();
        let input = result["input"].as_array().unwrap();

        assert_eq!(input.len(), 1);
        assert_eq!(input[0]["role"], "user");
        let content = input[0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "input_image");
        assert_eq!(content[0]["image_url"], "data:image/png;base64,aW1n");
    }

    #[test]
    fn test_multiple_images_preserved_in_order() {
        use crate::conversation::message::Message;

        let messages = vec![Message::user()
            .with_text("compare")
            .with_image("img1", "image/png")
            .with_image("img2", "image/jpeg")];

        let model_config = ModelConfig {
            model_name: "gpt-5.5".to_string(),
            context_limit: None,
            temperature: None,
            max_tokens: None,
            toolshim: false,
            toolshim_model: None,
            request_params: None,
            reasoning: None,
            supports_vision: Some(true),
            request_headers: None,
        };

        let result = create_responses_request(&model_config, "", &messages, &[]).unwrap();
        let input = result["input"].as_array().unwrap();

        assert_eq!(input[0]["role"], "user");
        let content = input[0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 3);
        assert_eq!(content[0]["type"], "input_text");
        assert_eq!(content[0]["text"], "compare");
        assert_eq!(content[1]["type"], "input_image");
        assert_eq!(content[1]["image_url"], "data:image/png;base64,img1");
        assert_eq!(content[2]["type"], "input_image");
        assert_eq!(content[2]["image_url"], "data:image/jpeg;base64,img2");
    }

    #[test]
    fn test_user_image_omitted_when_not_vision() {
        use crate::conversation::message::Message;

        let messages = vec![Message::user()
            .with_text("describe this image")
            .with_image("aW1hZ2VkYXRh", "image/png")];

        // Non-vision: explicit image content is replaced with a text placeholder
        // at format time — session history is untouched.
        let model_config = ModelConfig::new("o3-mini");
        let result = create_responses_request(&model_config, "", &messages, &[]).unwrap();
        let input = result["input"].as_array().unwrap();

        assert_eq!(input.len(), 1);
        assert_eq!(input[0]["role"], "user");
        let content = input[0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["type"], "input_text");
        assert_eq!(content[0]["text"], "describe this image");
        assert_eq!(content[1]["type"], "input_text");
        assert_eq!(
            content[1]["text"],
            "[image omitted: model does not support vision]"
        );

        let serialized = serde_json::to_string(&result).unwrap();
        assert!(!serialized.contains("input_image"));
        assert!(!serialized.contains("data:image"));
    }

    #[test]
    fn test_tool_response_image_omitted_when_not_vision() {
        use crate::conversation::message::Message;
        use rmcp::model::{CallToolResult, ContentBlock};

        let messages = vec![
            Message::user().with_content(MessageContentBlock::tool_response(
                "call_1",
                Ok(CallToolResult::success(vec![
                    ContentBlock::text("caption"),
                    ContentBlock::image("a+/=".to_string(), "image/png".to_string()),
                ])),
            )),
        ];

        // Non-vision: images in tool results are omitted from the output
        // string (with a note) instead of being emitted as input_image items.
        let model_config = ModelConfig::new("o3-mini");
        let result = create_responses_request(&model_config, "", &messages, &[]).unwrap();
        let input = result["input"].as_array().unwrap();

        assert_eq!(input[0]["type"], "function_call_output");
        let output = input[0]["output"].as_str().unwrap();
        assert!(output.contains("caption"));
        assert!(output.contains("[image omitted: model does not support vision]"));

        let serialized = serde_json::to_string(&result).unwrap();
        assert!(!serialized.contains("input_image"));
        assert!(!serialized.contains("data:image"));
    }

    #[test]
    fn test_assistant_text_uses_output_text_with_annotations() {
        use crate::conversation::message::Message;

        let messages = vec![Message::assistant().with_text("hello")];

        let model_config = ModelConfig {
            model_name: "gpt-5.5".to_string(),
            context_limit: None,
            temperature: None,
            max_tokens: None,
            toolshim: false,
            toolshim_model: None,
            request_params: None,
            reasoning: None,
            supports_vision: None,
            request_headers: None,
        };

        let result = create_responses_request(&model_config, "", &messages, &[]).unwrap();
        let input = result["input"].as_array().unwrap();

        assert_eq!(input[0]["role"], "assistant");
        assert_eq!(input[0]["content"][0]["type"], "output_text");
        assert_eq!(input[0]["content"][0]["text"], "hello");
        assert_eq!(input[0]["content"][0]["annotations"], serde_json::json!([]));
    }

    #[test]
    fn test_responses_api_to_message_sanitizes_unicode_tags() {
        let response: ResponsesApiResponse = serde_json::from_value(serde_json::json!({
            "id": "resp_1",
            "object": "response",
            "created_at": 0,
            "status": "completed",
            "model": "gpt-5.5",
            "output": [{
                "type": "message",
                "id": "msg_1",
                "status": "completed",
                "role": "assistant",
                "content": [
                    {"type": "output_text", "text": "visible\u{E0041}text"},
                    {"type": "refusal", "refusal": "cannot\u{E0042}help"}
                ]
            }]
        }))
        .unwrap();

        let message = responses_api_to_message(&response).unwrap();
        let text = message
            .content
            .iter()
            .filter_map(MessageContentBlock::as_text)
            .collect::<Vec<_>>();

        assert_eq!(text, vec!["visibletext", "cannothelp"]);
    }

    #[test]
    fn test_streaming_output_items_sanitize_unicode_tags() {
        let item: ResponseOutputItemInfo = serde_json::from_value(serde_json::json!({
            "type": "message",
            "id": "msg_1",
            "status": "completed",
            "role": "assistant",
            "content": [
                {"type": "output_text", "text": "visible\u{E0041}text"},
                {"type": "refusal", "refusal": "cannot\u{E0042}help"}
            ]
        }))
        .unwrap();

        let content = process_streaming_output_items(vec![item], false).unwrap();
        let text = content
            .iter()
            .filter_map(MessageContentBlock::as_text)
            .collect::<Vec<_>>();

        assert_eq!(text, vec!["visibletext", "cannothelp"]);
    }

    #[tokio::test]
    async fn test_streaming_deltas_sanitize_unicode_tags() -> anyhow::Result<()> {
        let lines = vec![
            format!(
                "data: {}",
                serde_json::json!({
                    "type": "response.created",
                    "sequence_number": 1,
                    "response": {
                        "id": "resp_1",
                        "object": "response",
                        "created_at": 0,
                        "status": "in_progress",
                        "model": "gpt-5.5",
                        "output": []
                    }
                })
            ),
            format!(
                "data: {}",
                serde_json::json!({
                    "type": "response.output_text.delta",
                    "sequence_number": 2,
                    "item_id": "msg_1",
                    "output_index": 0,
                    "content_index": 0,
                    "delta": "visible\u{E0041}te"
                })
            ),
            format!(
                "data: {}",
                serde_json::json!({
                    "type": "response.output_text.delta",
                    "sequence_number": 3,
                    "item_id": "msg_1",
                    "output_index": 0,
                    "content_index": 0,
                    "delta": "xt e"
                })
            ),
            format!(
                "data: {}",
                serde_json::json!({
                    "type": "response.output_text.delta",
                    "sequence_number": 4,
                    "item_id": "msg_1",
                    "output_index": 0,
                    "content_index": 0,
                    "delta": "\u{301}"
                })
            ),
            format!(
                "data: {}",
                serde_json::json!({
                    "type": "response.refusal.delta",
                    "sequence_number": 5,
                    "item_id": "msg_1",
                    "output_index": 0,
                    "content_index": 1,
                    "delta": "cannot\u{E0042}help"
                })
            ),
            "data: [DONE]".to_string(),
        ];
        let response_stream = tokio_stream::iter(lines.into_iter().map(Ok));
        let messages = responses_api_to_streaming_message(response_stream);
        futures::pin_mut!(messages);

        let mut text = Vec::new();
        while let Some(item) = messages.next().await {
            if let Some(message) = item?.0 {
                text.extend(
                    message
                        .content
                        .iter()
                        .filter_map(MessageContentBlock::as_text)
                        .map(str::to_owned),
                );
            }
        }

        assert_eq!(text.concat(), "visibletext e\u{301}cannothelp");
        Ok(())
    }

    #[test]
    fn test_refusal_content_block_deserializes_in_non_streaming_response() {
        let json = r#"{
            "id": "resp_1",
            "object": "response",
            "created_at": 0,
            "status": "completed",
            "model": "gpt-5.5",
            "output": [{
                "type": "message",
                "id": "msg_1",
                "status": "completed",
                "role": "assistant",
                "content": [{"type": "refusal", "refusal": "I cannot help with that request."}]
            }]
        }"#;

        let response: ResponsesApiResponse = serde_json::from_str(json).unwrap();
        let message = responses_api_to_message(&response).unwrap();
        assert_eq!(message.content.len(), 1);
        if let MessageContentBlock::Text(t) = &message.content[0] {
            assert_eq!(t.text, "I cannot help with that request.");
        } else {
            panic!("expected text content from refusal");
        }
    }

    #[test]
    fn test_refusal_content_part_deserializes_in_streaming_output() -> anyhow::Result<()> {
        let json = r#"{
            "type": "message",
            "id": "msg_1",
            "status": "completed",
            "role": "assistant",
            "content": [{"type": "refusal", "refusal": "I'm unable to assist."}]
        }"#;

        let item: ResponseOutputItemInfo = serde_json::from_str(json).unwrap();
        let content = process_streaming_output_items(vec![item], false)?;
        assert_eq!(content.len(), 1);
        if let MessageContentBlock::Text(t) = &content[0] {
            assert_eq!(t.text, "I'm unable to assist.");
        } else {
            panic!("expected text content from refusal");
        }

        Ok(())
    }

    #[test]
    fn test_refusal_delta_stream_event_deserializes() {
        let json = r#"{"type":"response.refusal.delta","sequence_number":5,"item_id":"msg_1","output_index":0,"content_index":0,"delta":"I cannot"}"#;

        let event: ResponsesStreamEvent = serde_json::from_str(json).unwrap();
        match event {
            ResponsesStreamEvent::RefusalDelta { delta, .. } => {
                assert_eq!(delta, "I cannot");
            }
            _ => panic!("expected RefusalDelta event"),
        }
    }

    #[test]
    fn test_streamed_refusal_not_duplicated_in_output_items() -> anyhow::Result<()> {
        let output_items = vec![ResponseOutputItemInfo::Message {
            id: Some("msg_1".to_string()),
            status: Some("completed".to_string()),
            role: "assistant".to_string(),
            content: vec![ContentBlockPart::Refusal {
                refusal: "I cannot help with that.".to_string(),
            }],
        }];

        let content = process_streaming_output_items(output_items.clone(), true)?;
        assert!(
            content.is_empty(),
            "refusal should be suppressed when already streamed"
        );

        let content = process_streaming_output_items(output_items, false)?;
        assert_eq!(
            content.len(),
            1,
            "refusal should appear in non-streaming path"
        );

        Ok(())
    }

    #[test]
    fn test_reasoning_text_content_part_deserializes() {
        let json = r#"{"type":"response.content_part.added","content_index":0,"item_id":"64cda83e-03b0-4e61-ab77-30d06ffe7f23","output_index":0,"part":{"type":"reasoning_text","text":"thinking..."},"sequence_number":3}"#;

        let event: ResponsesStreamEvent = serde_json::from_str(json).unwrap();
        match event {
            ResponsesStreamEvent::ContentBlockPartAdded { part, .. } => match part {
                ContentBlockPart::ReasoningText { text } => {
                    assert_eq!(text, "thinking...");
                }
                _ => panic!("expected ReasoningText part"),
            },
            _ => panic!("expected ContentBlockPartAdded event"),
        }
    }

    #[test]
    fn test_reasoning_text_surfaces_as_thinking_block() -> anyhow::Result<()> {
        let output_items = vec![ResponseOutputItemInfo::Message {
            id: Some("msg_1".to_string()),
            status: Some("completed".to_string()),
            role: "assistant".to_string(),
            content: vec![ContentBlockPart::ReasoningText {
                text: "thinking...".to_string(),
            }],
        }];

        let content = process_streaming_output_items(output_items, true)?;
        assert_eq!(content.len(), 1, "reasoning text should be kept");
        assert!(
            matches!(content[0], MessageContentBlock::Thinking(_)),
            "reasoning text should become a thinking block"
        );

        Ok(())
    }

    #[test]
    fn test_function_call_output_requires_call_id_or_id() {
        let output_items = vec![ResponseOutputItemInfo::FunctionCall {
            id: None,
            status: None,
            call_id: None,
            name: "shell".to_string(),
            arguments: "{}".to_string(),
        }];

        let error = process_streaming_output_items(output_items, false).unwrap_err();
        assert!(
            error.to_string().contains("missing call_id and id"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn test_streaming_output_items_sanitize_tool_arguments() -> anyhow::Result<()> {
        let output_items = vec![
            ResponseOutputItemInfo::Message {
                id: Some("msg_1".to_string()),
                status: Some("completed".to_string()),
                role: "assistant".to_string(),
                content: vec![ContentBlockPart::ToolCall {
                    id: "call_\u{E0041}1".to_string(),
                    name: "sh\u{E0041}ell".to_string(),
                    arguments: serde_json::to_string(
                        &json!({"prompt": "visible e\u{301}\u{E0041}text"}),
                    )?,
                }],
            },
            ResponseOutputItemInfo::FunctionCall {
                id: None,
                status: Some("completed".to_string()),
                call_id: Some("call_\u{E0042}2".to_string()),
                name: "sh\u{E0042}ell".to_string(),
                arguments: serde_json::to_string(
                    &json!({"prompt": "visible e\u{301}\u{E0042}text"}),
                )?,
            },
        ];

        let content = process_streaming_output_items(output_items, false)?;
        for (content, expected_id) in content.into_iter().zip(["call_1", "call_2"]) {
            let MessageContentBlock::ToolRequest(tool_request) = content else {
                panic!("expected tool request content");
            };
            assert_eq!(tool_request.id, expected_id);
            let tool_call = tool_request.tool_call.expect("expected valid tool call");
            assert_eq!(tool_call.name, "shell");
            assert_eq!(
                tool_call
                    .arguments
                    .expect("expected arguments")
                    .get("prompt"),
                Some(&json!("visible e\u{301}text"))
            );
        }

        Ok(())
    }

    #[test]
    fn test_streaming_output_items_reject_sanitized_tool_request_id_collisions(
    ) -> anyhow::Result<()> {
        let output_items = vec![
            ResponseOutputItemInfo::Message {
                id: Some("msg_1".to_string()),
                status: Some("completed".to_string()),
                role: "assistant".to_string(),
                content: vec![ContentBlockPart::ToolCall {
                    id: "call_1".to_string(),
                    name: "shell".to_string(),
                    arguments: "{}".to_string(),
                }],
            },
            ResponseOutputItemInfo::FunctionCall {
                id: None,
                status: Some("completed".to_string()),
                call_id: Some("call_\u{E0041}1".to_string()),
                name: "shell".to_string(),
                arguments: "{}".to_string(),
            },
        ];

        let error = process_streaming_output_items(output_items, false).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("duplicate ID after Unicode tag sanitization"),
            "unexpected error: {error}"
        );

        Ok(())
    }

    #[test]
    fn test_streaming_output_items_reject_sanitized_tool_argument_key_collisions(
    ) -> anyhow::Result<()> {
        let colliding_arguments = json!({
            "command": "visible",
            "comm\u{E0041}and": "hidden",
        });
        let output_items = [
            ResponseOutputItemInfo::Message {
                id: Some("msg_1".to_string()),
                status: Some("completed".to_string()),
                role: "assistant".to_string(),
                content: vec![ContentBlockPart::ToolCall {
                    id: "call_1".to_string(),
                    name: "shell".to_string(),
                    arguments: serde_json::to_string(&colliding_arguments)?,
                }],
            },
            ResponseOutputItemInfo::FunctionCall {
                id: None,
                status: Some("completed".to_string()),
                call_id: Some("call_2".to_string()),
                name: "shell".to_string(),
                arguments: serde_json::to_string(&colliding_arguments)?,
            },
        ];

        for output_item in output_items {
            let error = process_streaming_output_items(vec![output_item], false).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("duplicate key after Unicode tag sanitization"),
                "unexpected error: {error}"
            );
        }

        Ok(())
    }

    #[test]
    fn test_responses_request_sanitizes_replayed_function_call_names() {
        use crate::conversation::message::Message;

        let messages = vec![
            Message::assistant().with_tool_request(
                "call_agent",
                Ok(CallToolRequestParams::new("Crack Catcher")
                    .with_arguments(object!({"prompt": "verify the work"}))),
            ),
            Message::assistant().with_tool_request(
                "call_review_agent",
                Ok(CallToolRequestParams::new("@Review Agent")
                    .with_arguments(object!({"prompt": "check it"}))),
            ),
        ];

        let model_config = ModelConfig {
            model_name: "gpt-5.5".to_string(),
            context_limit: None,
            temperature: None,
            max_tokens: None,
            toolshim: false,
            toolshim_model: None,
            request_params: None,
            reasoning: None,
            supports_vision: None,
            request_headers: None,
        };

        let result = create_responses_request(&model_config, "", &messages, &[]).unwrap();
        let input = result["input"].as_array().unwrap();

        assert_eq!(input[0]["type"], "function_call");
        assert_eq!(input[0]["call_id"], "call_agent");
        assert_eq!(input[0]["name"], "Crack_Catcher");

        assert_eq!(input[1]["type"], "function_call");
        assert_eq!(input[1]["call_id"], "call_review_agent");
        assert_eq!(input[1]["name"], "_Review_Agent");
    }

    #[test]
    fn test_responses_request_limits_replayed_function_call_names() {
        use crate::conversation::message::Message;

        let messages = vec![Message::assistant().with_tool_request(
            "call_long_name",
            Ok(CallToolRequestParams::new("a".repeat(160))),
        )];
        let model_config = ModelConfig {
            model_name: "gpt-5.5".to_string(),
            context_limit: None,
            temperature: None,
            max_tokens: None,
            toolshim: false,
            toolshim_model: None,
            request_params: None,
            reasoning: None,
            supports_vision: None,
            request_headers: None,
        };

        let result = create_responses_request(&model_config, "", &messages, &[]).unwrap();
        let name = result["input"][0]["name"].as_str().unwrap();

        assert_eq!(name.len(), 128);
    }

    #[test]
    fn test_tool_request_error_emits_function_call_output() {
        use crate::conversation::message::Message;
        use rmcp::model::{ErrorCode, ErrorData};

        let messages = vec![Message::assistant().with_tool_request(
            "call_err1",
            Err(ErrorData {
                code: ErrorCode::INTERNAL_ERROR,
                message: "invalid arguments".into(),
                data: None,
            }),
        )];

        let model_config = ModelConfig {
            model_name: "gpt-5.5".to_string(),
            context_limit: None,
            temperature: None,
            max_tokens: None,
            toolshim: false,
            toolshim_model: None,
            request_params: None,
            reasoning: None,
            supports_vision: None,
            request_headers: None,
        };

        let result = create_responses_request(&model_config, "", &messages, &[]).unwrap();
        let input = result["input"].as_array().unwrap();

        assert_eq!(input.len(), 1);
        assert_eq!(input[0]["type"], "function_call_output");
        assert_eq!(input[0]["call_id"], "call_err1");
        assert!(input[0]["output"]
            .as_str()
            .unwrap()
            .contains("invalid arguments"));
    }
}
