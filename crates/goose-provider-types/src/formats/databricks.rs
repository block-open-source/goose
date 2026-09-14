use crate::cache_semantics::{apply_chat_payload_breakpoints, CacheSemantics};
use crate::conversation::message::{Message, MessageContentBlock};
use crate::formats::anthropic::{
    adaptive_output_effort, model_supports_temperature, requires_explicit_thinking_disable,
    thinking_block_is_stale, thinking_budget_tokens, thinking_type_for_provider, ThinkingType,
};
use crate::model::{is_goose_internal_request_param, ModelConfig};

use crate::documents::{
    convert_document, document_media_type_is_supported, unsupported_document_text, DocumentFormat,
    ASSISTANT_ROLE_REASON, UNSUPPORTED_MEDIA_TYPE_REASON,
};
use crate::formats::openai::{
    extract_reasoning_effort, is_openai_responses_model, is_valid_function_name,
    openai_reasoning_effort_for_thinking, sanitize_function_name, validate_tool_schemas,
};
use crate::images::{convert_image, detect_image_path, load_image_file, ImageFormat};
use crate::mcp_utils::extract_text_from_resource;
use anyhow::{anyhow, Error};
use rmcp::model::{object, CallToolRequestParams, ContentBlock, ErrorCode, ErrorData, Role, Tool};
use serde::Serialize;
use serde_json::{json, Value};
use std::borrow::Cow;

pub const DATABRICKS_PROVIDER_NAME: &str = "databricks";

#[derive(Serialize)]
struct DatabricksMessage {
    content: Value,
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

fn format_text_content(
    text: &str,
    image_format: &ImageFormat,
    supports_vision: bool,
) -> (Vec<Value>, bool) {
    let mut items = vec![json!({"type": "text", "text": text})];
    let has_image = if supports_vision {
        if let Some(path) = detect_image_path(text) {
            if let Ok(image) = load_image_file(path.as_ref()) {
                items.push(convert_image(&image, image_format));
            }
            true
        } else {
            false
        }
    } else {
        false
    };
    (items, has_image)
}

fn format_tool_response(
    response: &crate::conversation::message::ToolResponse,
    image_format: &ImageFormat,
    supports_vision: bool,
) -> Vec<DatabricksMessage> {
    let mut result = Vec::new();

    match &response.tool_result {
        Ok(call_result) => {
            let abridged: Vec<_> = call_result.content.to_vec();

            let mut tool_content = Vec::new();
            let mut image_messages = Vec::new();

            for content in abridged {
                match content {
                    ContentBlock::Image(image) => {
                        if supports_vision {
                            tool_content.push(ContentBlock::text(
                            "This tool result included an image that is uploaded in the next message.",
                        ));
                            image_messages.push(DatabricksMessage {
                                role: "user".to_string(),
                                content: [convert_image(&image, image_format)].into(),
                                tool_calls: None,
                                tool_call_id: None,
                            });
                        } else {
                            tool_content.push(ContentBlock::text(
                                "This tool result included an image that was omitted as the model does not support vision.",
                            ));
                        }
                    }
                    ContentBlock::Resource(resource) => {
                        let text = extract_text_from_resource(&resource.resource);
                        tool_content.push(ContentBlock::text(text));
                    }
                    _ => tool_content.push(content),
                }
            }

            let tool_response_content: Value = json!(tool_content
                .iter()
                .filter_map(|c| c.as_text().map(|t| t.text.clone()))
                .collect::<Vec<String>>()
                .join(" "));

            result.push(DatabricksMessage {
                content: tool_response_content,
                role: "tool".to_string(),
                tool_call_id: Some(response.id.clone()),
                tool_calls: None,
            });
            result.extend(image_messages);
        }
        Err(e) => {
            result.push(DatabricksMessage {
                role: "tool".to_string(),
                content: format!("The tool call returned the following error:\n{}", e).into(),
                tool_call_id: Some(response.id.clone()),
                tool_calls: None,
            });
        }
    }

    result
}

fn format_messages(
    messages: &[Message],
    image_format: &ImageFormat,
    current_model: Option<&str>,
    supports_vision: bool,
) -> Vec<DatabricksMessage> {
    let mut result = Vec::new();
    for message in messages {
        let thinking_is_stale = thinking_block_is_stale(message, current_model);
        let mut converted = DatabricksMessage {
            content: Value::Null,
            role: match message.role {
                Role::User => "user".to_string(),
                Role::Assistant => "assistant".to_string(),
            },
            tool_calls: None,
            tool_call_id: None,
        };

        let mut content_array = Vec::new();
        let mut has_tool_calls = false;
        let mut has_multiple_content = false;
        // Deferred so all tool-role messages stay consecutive (required by Claude via Databricks).
        let mut pending_image_messages: Vec<DatabricksMessage> = Vec::new();

        for content in &message.content {
            match content {
                MessageContentBlock::Text(text) => {
                    if !text.text.is_empty() {
                        let (items, multi) =
                            format_text_content(&text.text, image_format, supports_vision);
                        content_array.extend(items);
                        has_multiple_content |= multi;
                    }
                }
                MessageContentBlock::Thinking(content) => {
                    if !thinking_is_stale {
                        has_multiple_content = true;
                        content_array.push(json!({
                            "type": "reasoning",
                            "summary": [{
                                "type": "summary_text",
                                "text": content.thinking,
                                "signature": content.signature
                            }]
                        }));
                    }
                }
                MessageContentBlock::RedactedThinking(content) => {
                    if !thinking_is_stale {
                        has_multiple_content = true;
                        content_array.push(json!({
                            "type": "reasoning",
                            "summary": [{"type": "summary_encrypted_text", "data": content.data}]
                        }));
                    }
                }
                MessageContentBlock::ToolRequest(request) => {
                    has_tool_calls = true;
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

                            let tool_calls = converted.tool_calls.get_or_insert_default();
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

                            tool_calls.push(tool_call_json);
                        }
                        Err(_e) => {
                            // Mirror the OpenAI formatter: emitting the error as assistant
                            // text leaves no `tool_calls` entry, so the paired tool response
                            // orphans (a `role:"tool"` with no preceding assistant
                            // `tool_calls`) and strict APIs reject it. Emit a placeholder
                            // call with the same id; the error rides on the tool response.
                            let tool_calls = converted.tool_calls.get_or_insert_default();
                            tool_calls.push(json!({
                                "id": request.id,
                                "type": "function",
                                "function": {
                                    "name": "unparseable_tool_call",
                                    "arguments": "{}",
                                }
                            }));
                        }
                    }
                }
                MessageContentBlock::ToolResponse(response) => {
                    for msg in format_tool_response(response, image_format, supports_vision) {
                        if msg.role == "user" {
                            pending_image_messages.push(msg);
                        } else {
                            result.push(msg);
                        }
                    }
                }
                MessageContentBlock::Image(image) => {
                    if supports_vision {
                        content_array.push(convert_image(image, image_format));
                    } else {
                        content_array.push(json!({
                            "type": "text",
                            "text": "[image omitted: model does not support vision]"
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
                        content_array.push(convert_document(document, &DocumentFormat::OpenAi));
                    } else {
                        content_array.push(json!({
                            "type": "text",
                            "text": unsupported_document_text(document, UNSUPPORTED_MEDIA_TYPE_REASON)
                        }));
                    }
                }
                MessageContentBlock::SystemNotification(_)
                | MessageContentBlock::Error(_)
                | MessageContentBlock::ToolConfirmationRequest(_)
                | MessageContentBlock::ActionRequired(_) => {}
            }
        }

        result.extend(pending_image_messages);

        if !content_array.is_empty() {
            converted.content = if content_array.len() == 1
                && !has_multiple_content
                && content_array[0]["type"] == "text"
            {
                json!(content_array[0]["text"])
            } else {
                json!(content_array)
            };
        }

        if !content_array.is_empty() || has_tool_calls {
            result.push(converted);
        }
    }

    result
}

fn apply_claude_thinking_config(
    payload: &mut Value,
    provider_name: &str,
    model_config: &ModelConfig,
) {
    let obj = payload.as_object_mut().unwrap();

    match thinking_type_for_provider(provider_name, model_config) {
        ThinkingType::Adaptive => {
            obj.insert("thinking".to_string(), json!({ "type": "adaptive" }));
            obj.insert(
                "output_config".to_string(),
                json!({ "effort": adaptive_output_effort(model_config).to_string() }),
            );
            obj.insert(
                "max_completion_tokens".to_string(),
                json!(model_config.max_output_tokens()),
            );
        }
        ThinkingType::Enabled => {
            let budget_tokens = thinking_budget_tokens(model_config);
            let max_tokens = model_config.max_output_tokens() + budget_tokens;
            obj.insert("max_tokens".to_string(), json!(max_tokens));
            obj.insert(
                "thinking".to_string(),
                json!({
                    "type": "enabled",
                    "budget_tokens": budget_tokens
                }),
            );
            obj.insert("temperature".to_string(), json!(2));
        }
        ThinkingType::Disabled => {
            if requires_explicit_thinking_disable(provider_name, &model_config.model_name) {
                obj.insert("thinking".to_string(), json!({ "type": "disabled" }));
            }
            if model_supports_temperature(provider_name, model_config) {
                if let Some(temp) = model_config.temperature {
                    obj.insert("temperature".to_string(), json!(temp));
                }
            }
            obj.insert(
                "max_completion_tokens".to_string(),
                json!(model_config.max_output_tokens()),
            );
        }
    }
}

pub fn format_tools(tools: &[Tool], _model_name: &str) -> anyhow::Result<Vec<Value>> {
    let mut tool_names = std::collections::HashSet::new();
    let mut result = Vec::new();

    for tool in tools {
        if !tool_names.insert(&tool.name) {
            return Err(anyhow!("Duplicate tool name: {}", tool.name));
        }

        let has_properties = tool
            .input_schema
            .get("properties")
            .and_then(|v| v.as_object())
            .is_some_and(|p| !p.is_empty());

        // Databricks serving endpoints (including Gemini-backed ones) use the
        // OpenAI-compatible chat format, so tools always use "parameters" — not
        // the Google-native "parametersJsonSchema" field.
        let mut def = json!({
            "name": tool.name,
            "description": tool.description,
        });
        if has_properties {
            def["parameters"] = json!(tool.input_schema);
        }
        let function_def = def;

        result.push(json!({
            "type": "function",
            "function": function_def,
        }));
    }

    Ok(result)
}

/// Convert Databricks' API response to internal Message format
#[allow(clippy::too_many_lines)]
pub fn response_to_message(response: &Value) -> anyhow::Result<Message> {
    let original = &response["choices"][0]["message"];
    let mut content = Vec::new();

    // Handle array-based content
    if let Some(content_array) = original.get("content").and_then(|c| c.as_array()) {
        for content_item in content_array {
            match content_item.get("type").and_then(|t| t.as_str()) {
                Some("text") => {
                    if let Some(text) = content_item.get("text").and_then(|t| t.as_str()) {
                        content.push(MessageContentBlock::text(text));
                    }
                }
                Some("reasoning") => {
                    if let Some(summary_array) =
                        content_item.get("summary").and_then(|s| s.as_array())
                    {
                        for summary in summary_array {
                            match summary.get("type").and_then(|t| t.as_str()) {
                                Some("summary_text") => {
                                    let text = summary
                                        .get("text")
                                        .and_then(|t| t.as_str())
                                        .unwrap_or_default();
                                    let signature = summary
                                        .get("signature")
                                        .and_then(|s| s.as_str())
                                        .unwrap_or_default();
                                    content.push(MessageContentBlock::thinking(text, signature));
                                }
                                Some("summary_encrypted_text") => {
                                    if let Some(data) = summary.get("data").and_then(|d| d.as_str())
                                    {
                                        content.push(MessageContentBlock::redacted_thinking(data));
                                    }
                                }
                                _ => continue,
                            }
                        }
                    }
                }
                _ => continue,
            }
        }
    } else if let Some(text) = original.get("content").and_then(|t| t.as_str()) {
        // Handle legacy single string content
        content.push(MessageContentBlock::text(text));
    }

    // Handle tool calls
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

                if !is_valid_function_name(&function_name) {
                    let error = ErrorData {
                        code: ErrorCode::INVALID_REQUEST,
                        message: Cow::from(format!(
                            "The provided function name '{}' had invalid characters, it must match this regex [a-zA-Z0-9_-]+",
                            function_name
                        )),
                        data: None,
                    };
                    content.push(MessageContentBlock::tool_request(id, Err(error)));
                } else {
                    match crate::json::parse_tool_arguments(&arguments_str) {
                        Some(params) if params.is_object() => {
                            content.push(MessageContentBlock::tool_request(
                                id,
                                Ok(CallToolRequestParams::new(function_name)
                                    .with_arguments(object(params))),
                            ));
                        }
                        // Valid JSON but NOT an object (a bare array/string/number).
                        // Surface a tool error so the model retries instead of
                        // crashing the run (rmcp's `object()` debug-asserts).
                        Some(_) => {
                            let error = ErrorData {
                                code: ErrorCode::INVALID_PARAMS,
                                message: Cow::from(format!(
                                    "Tool arguments for {} (id {}) must be a JSON object. Raw arguments: '{}'",
                                    function_name, id, arguments_str
                                )),
                                data: None,
                            };
                            content.push(MessageContentBlock::tool_request(id, Err(error)));
                        }
                        None => {
                            let message_text =
                                crate::json::truncation_error_message(&arguments_str)
                                    .unwrap_or_else(|| {
                                        format!(
                                            "Could not interpret tool use parameters for id {id}"
                                        )
                                    });
                            let error = ErrorData {
                                code: ErrorCode::INVALID_PARAMS,
                                message: Cow::from(message_text),
                                data: None,
                            };
                            content.push(MessageContentBlock::tool_request(id, Err(error)));
                        }
                    }
                }
            }
        }
    }

    Ok(Message::new(
        Role::Assistant,
        chrono::Utc::now().timestamp(),
        content,
    ))
}

/// Check if the model name indicates a Claude/Anthropic model that supports cache control.
fn is_claude_model(model_name: &str) -> bool {
    model_name.contains("claude")
}

#[allow(clippy::too_many_lines)]
pub fn create_request(
    model_config: &ModelConfig,
    system: &str,
    messages: &[Message],
    tools: &[Tool],
    image_format: &ImageFormat,
) -> anyhow::Result<Value, Error> {
    create_request_for_provider(
        DATABRICKS_PROVIDER_NAME,
        model_config,
        system,
        messages,
        tools,
        image_format,
    )
}

pub fn create_request_for_provider(
    provider_name: &str,
    model_config: &ModelConfig,
    system: &str,
    messages: &[Message],
    tools: &[Tool],
    image_format: &ImageFormat,
) -> anyhow::Result<Value, Error> {
    if model_config.model_name.starts_with("o1-mini") {
        return Err(anyhow!(
            "o1-mini model is not currently supported since goose uses tool calling and o1-mini does not support it. Please use o1 or o3 models instead."
        ));
    }

    let (model_name, legacy_reasoning_effort) = extract_reasoning_effort(&model_config.model_name);
    let is_openai_reasoning_model = is_openai_responses_model(&model_name);
    let reasoning_effort = if is_openai_reasoning_model {
        model_config
            .thinking_effort()
            .map_or(legacy_reasoning_effort, |effort| {
                openai_reasoning_effort_for_thinking(&model_name, effort)
            })
    } else {
        None
    };

    let system_message = DatabricksMessage {
        role: "system".to_string(),
        content: system.into(),
        tool_calls: None,
        tool_call_id: None,
    };

    let model_supports_vision = model_config.supports_vision.unwrap_or_default();
    let messages_spec = format_messages(
        messages,
        image_format,
        Some(&model_config.model_name),
        model_supports_vision,
    );
    let mut tools_spec = if !tools.is_empty() {
        format_tools(tools, &model_config.model_name)?
    } else {
        vec![]
    };

    // Validate tool schemas
    validate_tool_schemas(&mut tools_spec);

    let mut messages_array = vec![system_message];
    messages_array.extend(messages_spec);

    let mut payload = json!({
        "model": model_name,
        "messages": messages_array
    });

    if let Some(effort) = reasoning_effort {
        payload
            .as_object_mut()
            .unwrap()
            .insert("reasoning_effort".to_string(), json!(effort));
    }

    if !tools_spec.is_empty() {
        payload
            .as_object_mut()
            .unwrap()
            .insert("tools".to_string(), json!(tools_spec));
    }

    if is_claude_model(&model_config.model_name) {
        apply_claude_thinking_config(&mut payload, provider_name, model_config);
    } else {
        // open ai reasoning models currently don't support temperature
        if !is_openai_reasoning_model && model_supports_temperature(provider_name, model_config) {
            if let Some(temp) = model_config.temperature {
                payload
                    .as_object_mut()
                    .unwrap()
                    .insert("temperature".to_string(), json!(temp));
            }
        }

        payload.as_object_mut().unwrap().insert(
            "max_completion_tokens".to_string(),
            json!(model_config.max_output_tokens()),
        );
    }

    if CacheSemantics::for_model("databricks", &model_config.model_name).uses_explicit_breakpoints()
        && !model_config.prompt_cache_disabled()
    {
        apply_chat_payload_breakpoints(&mut payload);
    }

    // Add request_params to the payload (e.g., anthropic_beta for extended context)
    if let Some(params) = &model_config.request_params {
        if let Some(obj) = payload.as_object_mut() {
            for (key, value) in params {
                if is_goose_internal_request_param(key) {
                    continue;
                }
                obj.insert(key.clone(), value.clone());
            }
        }
    }

    Ok(payload)
}

#[cfg(test)]
mod document_tests {
    use super::*;
    use crate::conversation::message::Message;

    fn format(messages: &[Message]) -> Vec<DatabricksMessage> {
        format_messages(messages, &ImageFormat::OpenAi, None, true)
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
            spec[0].content[0],
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
        assert_eq!(spec[0].role, "assistant");
        let text = spec[0].content.as_str().unwrap();
        assert!(text.contains("q3-report.pdf"), "{text}");
        assert!(text.contains("user messages"), "{text}");
        assert!(!text.contains("cGRmLWJ5dGVz"), "{text}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::message::{Message, MessageContent};
    use rmcp::model::CallToolResult;
    use rmcp::object;
    use serde_json::json;

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
        let spec = format_messages(&[message], &ImageFormat::OpenAi, None, false);

        assert_eq!(spec.len(), 1);
        assert_eq!(spec[0].role, "user");
        assert_eq!(spec[0].content, "Hello");
        Ok(())
    }

    #[test]
    fn keeps_reasoning_block_from_the_same_model() {
        use crate::conversation::message::InferenceMetadata;
        let message = Message::assistant()
            .with_content(MessageContent::thinking("internal", "sig-xyz"))
            .with_text("answer")
            .with_inference(InferenceMetadata {
                provider: "databricks".to_string(),
                requested_model: "databricks-claude-opus-4-1".to_string(),
                resolved_model: None,
                provider_session_id: None,
            });

        let spec = format_messages(
            &[message],
            &ImageFormat::OpenAi,
            Some("databricks-claude-opus-4-1"),
            false,
        );
        let has_reasoning = spec[0]
            .content
            .as_array()
            .map(|a| a.iter().any(|c| c["type"] == "reasoning"))
            .unwrap_or(false);
        assert!(has_reasoning, "same-model reasoning must be kept");
    }

    #[test]
    fn drops_reasoning_block_from_a_different_model() {
        use crate::conversation::message::InferenceMetadata;
        let message = Message::assistant()
            .with_content(MessageContent::thinking("internal", "sig-xyz"))
            .with_text("answer")
            .with_inference(InferenceMetadata {
                provider: "databricks".to_string(),
                requested_model: "databricks-claude-opus-4-1".to_string(),
                resolved_model: None,
                provider_session_id: None,
            });

        let spec = format_messages(
            &[message],
            &ImageFormat::OpenAi,
            Some("databricks-claude-sonnet-4-5"),
            false,
        );
        let has_reasoning = spec[0]
            .content
            .as_array()
            .map(|a| a.iter().any(|c| c["type"] == "reasoning"))
            .unwrap_or(false);
        assert!(!has_reasoning, "stale reasoning block must be dropped");
        assert_eq!(spec[0].content, Value::String("answer".to_string()));
    }

    #[test]
    fn keeps_reasoning_when_endpoint_matches_despite_upstream_resolved_name() {
        use crate::conversation::message::InferenceMetadata;
        let message = Message::assistant()
            .with_content(MessageContent::thinking("internal", "sig-xyz"))
            .with_text("answer")
            .with_inference(InferenceMetadata {
                provider: "databricks".to_string(),
                requested_model: "databricks-claude-opus-4-1".to_string(),
                resolved_model: Some("claude-opus-4.1".to_string()),
                provider_session_id: None,
            });

        let spec = format_messages(
            &[message],
            &ImageFormat::OpenAi,
            Some("databricks-claude-opus-4-1"),
            false,
        );
        let has_reasoning = spec[0]
            .content
            .as_array()
            .map(|a| a.iter().any(|c| c["type"] == "reasoning"))
            .unwrap_or(false);
        assert!(
            has_reasoning,
            "same-endpoint reasoning must be kept even when resolved_model differs"
        );
    }

    #[test]
    fn keeps_reasoning_when_current_model_matches_upstream_resolved_name() {
        use crate::conversation::message::InferenceMetadata;
        let message = Message::assistant()
            .with_content(MessageContent::thinking("internal", "sig-xyz"))
            .with_text("answer")
            .with_inference(InferenceMetadata {
                provider: "databricks".to_string(),
                requested_model: "my-claude-endpoint".to_string(),
                resolved_model: Some("claude-opus-4.1".to_string()),
                provider_session_id: None,
            });

        let spec = format_messages(
            &[message],
            &ImageFormat::OpenAi,
            Some("claude-opus-4.1"),
            false,
        );
        let has_reasoning = spec[0]
            .content
            .as_array()
            .map(|a| a.iter().any(|c| c["type"] == "reasoning"))
            .unwrap_or(false);
        assert!(
            has_reasoning,
            "reasoning must be kept when current_model matches the upstream resolved_model"
        );
    }

    #[test]
    fn test_format_messages_sanitizes_resource_tool_response() {
        let message = Message::user().with_tool_response(
            "tool1",
            Ok(CallToolResult::success(vec![ContentBlock::embedded_text(
                "file:///result.txt",
                "visible\u{E0041}text",
            )])),
        );

        let spec = format_messages(&[message], &ImageFormat::OpenAi, None, false);

        assert_eq!(spec[0].content, "visibletext");
    }

    #[test]
    fn test_format_tools() -> anyhow::Result<()> {
        let tool = Tool::new(
            "test_tool",
            "A test tool",
            object!({
                "$schema": "http://json-schema.org/draft-07/schema#",
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

        let spec = format_tools(std::slice::from_ref(&tool), "gpt-4o")?;
        assert_eq!(
            spec[0]["function"]["parameters"]["$schema"],
            "http://json-schema.org/draft-07/schema#"
        );

        // Databricks Gemini endpoints use OpenAI-compatible format, so tools use
        // "parameters" (not "parametersJsonSchema") regardless of model family.
        let spec = format_tools(std::slice::from_ref(&tool), "gemini-2-5-flash")?;
        assert!(spec[0]["function"].get("parametersJsonSchema").is_none());
        assert!(spec[0]["function"].get("parameters").is_some());
        assert_eq!(spec[0]["function"]["parameters"]["type"], "object");

        let spec = format_tools(&[tool], "databricks-gemini-3-pro")?;
        assert!(spec[0]["function"].get("parametersJsonSchema").is_none());
        assert!(spec[0]["function"].get("parameters").is_some());
        assert_eq!(spec[0]["function"]["parameters"]["type"], "object");

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

        let tool_id = if let MessageContentBlock::ToolRequest(request) = &messages[2].content[0] {
            &request.id
        } else {
            panic!("should be tool request");
        };

        messages.push(Message::user().with_tool_response(
            tool_id,
            Ok(CallToolResult::success(vec![ContentBlock::text("Result")])),
        ));

        let as_value = serde_json::to_value(format_messages(
            &messages,
            &ImageFormat::OpenAi,
            None,
            false,
        ))
        .unwrap();
        let spec = as_value.as_array().unwrap();

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

        let tool_id = if let MessageContentBlock::ToolRequest(request) = &messages[0].content[0] {
            &request.id
        } else {
            panic!("should be tool request");
        };

        messages.push(Message::user().with_tool_response(
            tool_id,
            Ok(CallToolResult::success(vec![ContentBlock::text("Result")])),
        ));

        let as_value = serde_json::to_value(format_messages(
            &messages,
            &ImageFormat::OpenAi,
            None,
            false,
        ))
        .unwrap();
        let spec = as_value.as_array().unwrap();

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

        let result = format_tools(&[tool1, tool2], "gpt-4o");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Duplicate tool name"));

        Ok(())
    }

    #[test]
    fn test_format_messages_with_image_path() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let png_path = temp_dir.path().join("test.png");
        let png_data = [
            0x89, 0x50, 0x4E, 0x47, // PNG magic number
            0x0D, 0x0A, 0x1A, 0x0A, // PNG header
            0x00, 0x00, 0x00, 0x0D, // Rest of fake PNG data
        ];
        std::fs::write(&png_path, png_data)?;
        let png_path_str = png_path.to_str().unwrap();

        // Create message with image path
        let message = Message::user().with_text(format!("Here is an image: {}", png_path_str));
        let as_value = serde_json::to_value(format_messages(
            &[message],
            &ImageFormat::OpenAi,
            None,
            true,
        ))
        .unwrap();
        let spec = as_value.as_array().unwrap();

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

        Ok(())
    }

    #[test]
    fn test_format_messages_with_image_path_passthrough_when_not_vision() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let png_path = temp_dir.path().join("test.png");
        let png_data = [
            0x89, 0x50, 0x4E, 0x47, // PNG magic number
            0x0D, 0x0A, 0x1A, 0x0A, // PNG header
            0x00, 0x00, 0x00, 0x0D, // Rest of fake PNG data
        ];
        std::fs::write(&png_path, png_data)?;
        let png_path_str = png_path.to_str().unwrap();

        // Without vision support the path must pass through as plain text —
        // a non-vision model can forward it to a vision subagent instead of
        // 400ing on an injected image_url block.
        let message = Message::user().with_text(format!("Here is an image: {}", png_path_str));
        let as_value = serde_json::to_value(format_messages(
            &[message],
            &ImageFormat::OpenAi,
            None,
            false,
        ))
        .unwrap();
        let spec = as_value.as_array().unwrap();

        assert_eq!(spec.len(), 1);
        assert_eq!(spec[0]["role"], "user");
        let content = spec[0]["content"].as_str().unwrap();
        assert!(content.contains(png_path_str));
        assert!(!content.contains("image_url"));
        assert!(!content.contains("data:image"));

        Ok(())
    }

    #[test]
    fn test_format_messages_with_image_block_passthrough_when_not_vision() -> anyhow::Result<()> {
        let message = Message::user().with_image("aW1hZ2VkYXRh", "image/png");

        // Non-vision: explicit image content is replaced with a text placeholder
        // at format time — session history is untouched.
        let as_value = serde_json::to_value(format_messages(
            std::slice::from_ref(&message),
            &ImageFormat::OpenAi,
            None,
            false,
        ))
        .unwrap();
        let spec = as_value.as_array().unwrap();
        let content = spec[0]["content"].as_str().unwrap();
        assert_eq!(content, "[image omitted: model does not support vision]");
        assert!(!content.contains("image_url"));

        // Vision: the image is converted to an image_url block (existing behavior).
        let as_value = serde_json::to_value(format_messages(
            &[message],
            &ImageFormat::OpenAi,
            None,
            true,
        ))
        .unwrap();
        let spec = as_value.as_array().unwrap();
        let content = spec[0]["content"].as_array().unwrap();
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

        // Non-vision: no separate user image message is emitted — this is what
        // un-bricks sessions (a converted image in the next request 400s).
        let as_value = serde_json::to_value(format_messages(
            std::slice::from_ref(&tool_response),
            &ImageFormat::OpenAi,
            None,
            false,
        ))
        .unwrap();
        let serialized = as_value.to_string();
        assert!(!serialized.contains("image_url"));
        assert!(serialized.contains(
            "This tool result included an image that was omitted as the model does not support vision."
        ));

        // Vision: the separate user image message IS emitted (existing behavior).
        let as_value = serde_json::to_value(format_messages(
            &[tool_response],
            &ImageFormat::OpenAi,
            None,
            true,
        ))
        .unwrap();
        let serialized = as_value.to_string();
        assert!(serialized.contains("image_url"));
        assert!(serialized
            .contains("This tool result included an image that is uploaded in the next message."));

        Ok(())
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
    fn test_response_to_message_invalid_func_name() -> anyhow::Result<()> {
        let mut response: Value = serde_json::from_str(OPENAI_TOOL_USE_RESPONSE)?;
        response["choices"][0]["message"]["tool_calls"][0]["function"]["name"] =
            json!("invalid fn");

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
                _ => panic!("Expected ToolNotFound error"),
            }
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
    fn test_create_request_gpt_4o() -> anyhow::Result<()> {
        // Test default medium reasoning effort for O3 model
        let model_config = ModelConfig {
            model_name: "gpt-4o".to_string(),
            context_limit: Some(4096),
            temperature: None,
            max_tokens: Some(1024),
            toolshim: false,
            toolshim_model: None,
            request_params: None,
            reasoning: None,
            supports_vision: None,
            request_headers: None,
        };
        let request = create_request(&model_config, "system", &[], &[], &ImageFormat::OpenAi)?;
        let obj = request.as_object().unwrap();
        let expected = json!({
            "model": "gpt-4o",
            "messages": [
                {
                    "role": "system",
                    "content": "system"
                }
            ],
            "max_completion_tokens": 1024
        });

        for (key, value) in expected.as_object().unwrap() {
            assert_eq!(obj.get(key).unwrap(), value);
        }

        Ok(())
    }

    #[test]
    fn test_create_request_reasoning_effort() -> anyhow::Result<()> {
        let mut params = std::collections::HashMap::new();
        params.insert("thinking_effort".to_string(), serde_json::json!("high"));
        let model_config = ModelConfig {
            model_name: "o3-mini".to_string(),
            context_limit: Some(4096),
            temperature: None,
            max_tokens: Some(1024),
            toolshim: false,
            toolshim_model: None,
            request_params: Some(params),
            reasoning: None,
            supports_vision: None,
            request_headers: None,
        };
        let request = create_request(&model_config, "system", &[], &[], &ImageFormat::OpenAi)?;
        assert_eq!(request["reasoning_effort"], "high");
        Ok(())
    }

    #[test]
    fn test_create_request_off_effort_uses_low() -> anyhow::Result<()> {
        let mut params = std::collections::HashMap::new();
        params.insert("thinking_effort".to_string(), serde_json::json!("off"));
        let model_config = ModelConfig {
            model_name: "databricks-o3-mini".to_string(),
            context_limit: Some(4096),
            temperature: None,
            max_tokens: Some(1024),
            toolshim: false,
            toolshim_model: None,
            request_params: Some(params),
            reasoning: None,
            supports_vision: None,
            request_headers: None,
        };
        let request = create_request(&model_config, "system", &[], &[], &ImageFormat::OpenAi)?;
        assert_eq!(request["reasoning_effort"], "low");
        assert!(request.get("thinking_effort").is_none());
        Ok(())
    }

    #[test]
    fn test_create_request_max_effort_uses_supported_level() -> anyhow::Result<()> {
        let mut params = std::collections::HashMap::new();
        params.insert("thinking_effort".to_string(), serde_json::json!("max"));
        let model_config = ModelConfig {
            model_name: "databricks-gpt-5.2-pro".to_string(),
            context_limit: Some(4096),
            temperature: None,
            max_tokens: Some(1024),
            toolshim: false,
            toolshim_model: None,
            request_params: Some(params),
            reasoning: None,
            supports_vision: None,
            request_headers: None,
        };
        let request = create_request(&model_config, "system", &[], &[], &ImageFormat::OpenAi)?;
        assert_eq!(request["reasoning_effort"], "high");
        assert!(request.get("thinking_effort").is_none());
        Ok(())
    }

    #[test]
    fn test_create_request_reasoning_effort_xhigh() -> anyhow::Result<()> {
        let model_config = ModelConfig {
            model_name: "o3-xhigh".to_string(),
            context_limit: Some(4096),
            temperature: None,
            max_tokens: Some(1024),
            toolshim: false,
            toolshim_model: None,
            request_params: None,
            reasoning: None,
            supports_vision: None,
            request_headers: None,
        };
        let request = create_request(&model_config, "system", &[], &[], &ImageFormat::OpenAi)?;
        assert_eq!(request["model"], "o3");
        assert_eq!(request["reasoning_effort"], "xhigh");
        Ok(())
    }

    #[test]
    fn test_create_request_reasoning_effort_none() -> anyhow::Result<()> {
        let model_config = ModelConfig {
            model_name: "o3-none".to_string(),
            context_limit: Some(4096),
            temperature: None,
            max_tokens: Some(1024),
            toolshim: false,
            toolshim_model: None,
            request_params: None,
            reasoning: None,
            supports_vision: None,
            request_headers: None,
        };
        let request = create_request(&model_config, "system", &[], &[], &ImageFormat::OpenAi)?;
        assert_eq!(request["model"], "o3");
        assert_eq!(request["reasoning_effort"], "none");
        Ok(())
    }

    #[test]
    fn test_create_request_reasoning_effort_for_prefixed_gpt5_model() -> anyhow::Result<()> {
        let model_config = ModelConfig {
            model_name: "databricks-gpt-5.4-high".to_string(),
            context_limit: Some(4096),
            temperature: None,
            max_tokens: Some(1024),
            toolshim: false,
            toolshim_model: None,
            request_params: None,
            reasoning: None,
            supports_vision: None,
            request_headers: None,
        };
        let request = create_request(&model_config, "system", &[], &[], &ImageFormat::OpenAi)?;
        assert_eq!(request["model"], "databricks-gpt-5.4");
        assert_eq!(request["reasoning_effort"], "high");
        Ok(())
    }

    #[test]
    fn test_create_request_one_shot_claude() -> anyhow::Result<()> {
        let model_config = ModelConfig::new("databricks-claude-sonnet-4-5")
            .with_merged_request_params(std::collections::HashMap::from([
                ("anthropic_beta".to_string(), serde_json::json!(["ctx-1m"])),
                ("disable_prompt_cache".to_string(), serde_json::json!(true)),
            ]));
        let messages = vec![Message::user().with_text("Summarize the conversation above.")];

        let request = create_request(
            &model_config,
            "system",
            &messages,
            &[],
            &ImageFormat::OpenAi,
        )?;

        assert_eq!(request["anthropic_beta"], serde_json::json!(["ctx-1m"]));
        assert!(request.get("disable_prompt_cache").is_none());
        assert!(!request.to_string().contains("cache_control"));

        Ok(())
    }

    #[test]
    fn test_create_request_adaptive_thinking_for_46_models() -> anyhow::Result<()> {
        let mut model_config = ModelConfig::new("databricks-claude-opus-4-6");
        model_config.max_tokens = Some(4096);
        let mut params = std::collections::HashMap::new();
        params.insert("thinking_effort".to_string(), serde_json::json!("low"));
        model_config.request_params = Some(params);

        let request = create_request(&model_config, "system", &[], &[], &ImageFormat::OpenAi)?;

        assert_eq!(request["thinking"]["type"], "adaptive");
        assert_eq!(request["output_config"]["effort"], "low");
        assert!(request.get("temperature").is_none());
        assert_eq!(request["max_completion_tokens"], 4096);
        assert!(request.get("max_tokens").is_none());

        Ok(())
    }

    #[test]
    fn test_create_request_adaptive_thinking_for_new_anthropic_models() -> anyhow::Result<()> {
        let _guard = env_lock::lock_env([("GOOSE_THINKING_EFFORT", None::<&str>)]);

        for name in [
            "databricks-claude-opus-4-7",
            "databricks-claude-opus-4-8",
            "databricks-claude-fable-5",
            "global.anthropic.claude-fable-5",
        ] {
            let mut model_config = ModelConfig::new(name);
            model_config.max_tokens = Some(4096);
            let mut params = std::collections::HashMap::new();
            params.insert("thinking_effort".to_string(), serde_json::json!("high"));
            model_config.request_params = Some(params);

            let request = create_request(&model_config, "system", &[], &[], &ImageFormat::OpenAi)?;

            assert_eq!(request["thinking"]["type"], "adaptive", "{name}");
            assert!(request.get("temperature").is_none(), "{name}");
            assert_eq!(request["max_completion_tokens"], 4096, "{name}");
            assert!(request.get("max_tokens").is_none(), "{name}");
        }

        Ok(())
    }

    #[test]
    fn test_create_request_always_on_adaptive_off_effort_falls_back_to_high() -> anyhow::Result<()>
    {
        let _guard = env_lock::lock_env([("GOOSE_THINKING_EFFORT", None::<&str>)]);
        let mut model_config = ModelConfig::new("databricks-claude-fable-5");
        model_config.max_tokens = Some(4096);
        let mut params = std::collections::HashMap::new();
        params.insert("thinking_effort".to_string(), serde_json::json!("off"));
        model_config.request_params = Some(params);

        let request = create_request(&model_config, "system", &[], &[], &ImageFormat::OpenAi)?;

        assert_eq!(request["thinking"]["type"], "adaptive");
        assert_eq!(request["output_config"]["effort"], "high");

        Ok(())
    }

    #[test]
    fn test_create_request_enabled_thinking_with_budget() -> anyhow::Result<()> {
        let mut model_config = ModelConfig::new("databricks-claude-sonnet-4.5");
        model_config.max_tokens = Some(4096);
        let mut params = std::collections::HashMap::new();
        params.insert("thinking_effort".to_string(), serde_json::json!("high"));
        model_config.request_params = Some(params);

        let request = create_request(&model_config, "system", &[], &[], &ImageFormat::OpenAi)?;

        assert_eq!(request["thinking"]["type"], "enabled");
        assert_eq!(request["thinking"]["budget_tokens"], 16000);
        assert_eq!(request["max_tokens"], 20096);
        assert_eq!(request["temperature"], 2);
        assert!(request.get("max_completion_tokens").is_none());

        Ok(())
    }

    #[test]
    fn test_create_request_enabled_thinking_budget_tracks_effort() -> anyhow::Result<()> {
        for (effort, expected_budget) in [
            ("low", 4000),
            ("medium", 10000),
            ("high", 16000),
            ("max", 32000),
        ] {
            let mut model_config = ModelConfig::new("databricks-claude-sonnet-4.5");
            model_config.max_tokens = Some(4096);
            let mut params = std::collections::HashMap::new();
            params.insert("thinking_effort".to_string(), serde_json::json!(effort));
            model_config.request_params = Some(params);

            let request = create_request(&model_config, "system", &[], &[], &ImageFormat::OpenAi)?;

            assert_eq!(request["thinking"]["type"], "enabled");
            assert_eq!(request["thinking"]["budget_tokens"], expected_budget);
            assert_eq!(request["max_tokens"], 4096 + expected_budget);
        }

        Ok(())
    }

    #[test]
    fn test_response_to_message_claude_thinking() -> anyhow::Result<()> {
        let response = json!({
            "model": "us.anthropic.claude-sonnet-4-5-20250929-v1:0",
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": [
                        {
                            "type": "reasoning",
                            "summary": [
                                {
                                    "type": "summary_text",
                                    "text": "Test thinking content",
                                    "signature": "test-signature"
                                }
                            ]
                        },
                        {
                            "type": "text",
                            "text": "Regular text content"
                        }
                    ]
                },
                "index": 0,
                "finish_reason": "stop"
            }]
        });

        let message = response_to_message(&response)?;
        assert_eq!(message.content.len(), 2);

        if let MessageContentBlock::Thinking(thinking) = &message.content[0] {
            assert_eq!(thinking.thinking, "Test thinking content");
            assert_eq!(thinking.signature, "test-signature");
        } else {
            panic!("Expected Thinking content");
        }

        if let MessageContentBlock::Text(text) = &message.content[1] {
            assert_eq!(text.text, "Regular text content");
        } else {
            panic!("Expected Text content");
        }

        Ok(())
    }

    #[test]
    fn test_response_to_message_claude_encrypted_thinking() -> anyhow::Result<()> {
        let response = json!({
            "model": "claude-sonnet-4-5-20250929",
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": [
                        {
                            "type": "reasoning",
                            "summary": [
                                {
                                    "type": "summary_encrypted_text",
                                    "data": "E23sQFCkYIARgCKkATCHitsdf327Ber3v4NYUq2"
                                }
                            ]
                        },
                        {
                            "type": "text",
                            "text": "Regular text content"
                        }
                    ]
                },
                "index": 0,
                "finish_reason": "stop"
            }]
        });

        let message = response_to_message(&response)?;
        assert_eq!(message.content.len(), 2);

        if let MessageContentBlock::RedactedThinking(redacted) = &message.content[0] {
            assert_eq!(redacted.data, "E23sQFCkYIARgCKkATCHitsdf327Ber3v4NYUq2");
        } else {
            panic!("Expected RedactedThinking content");
        }

        if let MessageContentBlock::Text(text) = &message.content[1] {
            assert_eq!(text.text, "Regular text content");
        } else {
            panic!("Expected Text content");
        }

        Ok(())
    }

    #[test]
    fn test_format_messages_tool_request_with_none_arguments() -> anyhow::Result<()> {
        // Test that tool calls with None arguments are formatted as "{}" string
        let message = Message::assistant()
            .with_tool_request("tool1", Ok(CallToolRequestParams::new("test_tool")));

        let spec = format_messages(&[message], &ImageFormat::OpenAi, None, false);
        let as_value = serde_json::to_value(spec)?;
        let spec_array = as_value.as_array().unwrap();

        assert_eq!(spec_array.len(), 1);
        assert_eq!(spec_array[0]["role"], "assistant");
        assert!(spec_array[0]["tool_calls"].is_array());

        let tool_call = &spec_array[0]["tool_calls"][0];
        assert_eq!(tool_call["id"], "tool1");
        assert_eq!(tool_call["type"], "function");
        assert_eq!(tool_call["function"]["name"], "test_tool");
        // This should be the string "{}", not null
        assert_eq!(tool_call["function"]["arguments"], "{}");

        Ok(())
    }

    #[test]
    fn format_messages_post_parse_error_history_is_wellformed() -> anyhow::Result<()> {
        // An unparseable tool call (ToolRequest(Err)) paired with its error tool
        // response must not serialize as an orphan role:"tool" message.
        use rmcp::model::{ErrorCode, ErrorData};
        let err = ErrorData::new(
            ErrorCode::INVALID_PARAMS,
            "Tool arguments for id call_bad must be a JSON object".to_string(),
            None,
        );
        let request_msg = Message::assistant().with_tool_request("call_bad", Err(err.clone()));
        let mut final_resp = Message::user();
        final_resp.add_tool_response_with_metadata("call_bad", Err(err), None);
        let messages = vec![
            Message::user().with_text("do the thing"),
            request_msg,
            final_resp,
        ];

        let spec = serde_json::to_value(format_messages(
            &messages,
            &ImageFormat::OpenAi,
            None,
            false,
        ))?;
        let mut open = std::collections::HashSet::new();
        for m in spec.as_array().unwrap() {
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

        let spec = format_messages(&[message], &ImageFormat::OpenAi, None, false);
        let as_value = serde_json::to_value(spec)?;
        let spec_array = as_value.as_array().unwrap();

        assert_eq!(spec_array.len(), 1);
        assert_eq!(spec_array[0]["role"], "assistant");
        assert!(spec_array[0]["tool_calls"].is_array());

        let tool_call = &spec_array[0]["tool_calls"][0];
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
    fn test_is_claude_model() {
        assert!(is_claude_model("databricks-claude-sonnet-4"));
        assert!(is_claude_model("databricks-claude-sonnet-4.5"));
        assert!(is_claude_model("claude-sonnet-4"));
        assert!(is_claude_model("goose-claude-sonnet"));
        assert!(!is_claude_model("gpt-4o"));
        assert!(!is_claude_model("gemini-2-5-flash"));
        assert!(!is_claude_model("databricks-meta-llama-3-3-70b"));
    }

    #[test]
    fn test_format_messages_with_thought_signature_metadata() -> anyhow::Result<()> {
        let mut metadata = serde_json::Map::new();
        metadata.insert(
            "thoughtSignature".to_string(),
            json!("sig_abc123_test_signature"),
        );

        let message = Message::assistant().with_tool_request_with_metadata(
            "tool1",
            Ok(CallToolRequestParams::new("test_tool").with_arguments(object!({"param": "value"}))),
            Some(&metadata),
            None,
        );

        let spec = format_messages(&[message], &ImageFormat::OpenAi, None, false);
        let as_value = serde_json::to_value(spec)?;
        let spec_array = as_value.as_array().unwrap();

        assert_eq!(spec_array.len(), 1);
        let tool_call = &spec_array[0]["tool_calls"][0];
        assert_eq!(tool_call["id"], "tool1");
        assert_eq!(tool_call["function"]["name"], "test_tool");
        assert_eq!(tool_call["thoughtSignature"], "sig_abc123_test_signature");

        Ok(())
    }

    #[test]
    fn test_create_request_claude_has_cache_control() -> anyhow::Result<()> {
        let model_config = ModelConfig {
            model_name: "databricks-claude-sonnet-4".to_string(),
            context_limit: Some(200000),
            temperature: None,
            max_tokens: Some(8192),
            toolshim: false,
            toolshim_model: None,
            request_params: None,
            reasoning: None,
            supports_vision: None,
            request_headers: None,
        };

        let messages = vec![
            Message::user().with_text("Hello"),
            Message::assistant().with_text("Hi there!"),
            Message::user().with_text("How are you?"),
        ];

        let tool = Tool::new(
            "test_tool",
            "A test tool",
            object!({
                "type": "object",
                "properties": {}
            }),
        );

        let request = create_request(
            &model_config,
            "You are helpful",
            &messages,
            &[tool],
            &ImageFormat::OpenAi,
        )?;

        // Verify system message has cache_control
        let messages_arr = request["messages"].as_array().unwrap();
        let system_msg = &messages_arr[0];
        assert!(system_msg["content"].is_array());
        assert_eq!(
            system_msg["content"][0]["cache_control"]["type"],
            "ephemeral"
        );

        // Verify last tool has cache_control
        let tools = request["tools"].as_array().unwrap();
        assert_eq!(tools[0]["function"]["cache_control"]["type"], "ephemeral");

        Ok(())
    }

    #[test]
    fn test_create_request_non_claude_no_cache_control() -> anyhow::Result<()> {
        let model_config = ModelConfig {
            model_name: "gpt-4o".to_string(),
            context_limit: Some(128000),
            temperature: None,
            max_tokens: Some(4096),
            toolshim: false,
            toolshim_model: None,
            request_params: None,
            reasoning: None,
            supports_vision: None,
            request_headers: None,
        };

        let messages = vec![Message::user().with_text("Hello")];

        let tool = Tool::new(
            "test_tool",
            "A test tool",
            object!({
                "type": "object",
                "properties": {}
            }),
        );

        let request = create_request(
            &model_config,
            "You are helpful",
            &messages,
            &[tool],
            &ImageFormat::OpenAi,
        )?;

        // Verify system message does NOT have cache_control (it's a plain string)
        let messages_arr = request["messages"].as_array().unwrap();
        let system_msg = &messages_arr[0];
        assert!(system_msg["content"].is_string());

        // Verify tool does NOT have cache_control
        let tools = request["tools"].as_array().unwrap();
        assert!(tools[0]["function"].get("cache_control").is_none());

        Ok(())
    }

    #[test]
    fn test_format_messages_with_multiple_metadata_fields() -> anyhow::Result<()> {
        let mut metadata = serde_json::Map::new();
        metadata.insert("thoughtSignature".to_string(), json!("sig_top_level"));
        metadata.insert(
            "extra_content".to_string(),
            json!({
                "google": {
                    "thought_signature": "sig_nested"
                }
            }),
        );
        metadata.insert("custom_field".to_string(), json!("custom_value"));

        let message = Message::assistant().with_tool_request_with_metadata(
            "tool1",
            Ok(CallToolRequestParams::new("test_tool")),
            Some(&metadata),
            None,
        );

        let spec = format_messages(&[message], &ImageFormat::OpenAi, None, false);
        let as_value = serde_json::to_value(spec)?;
        let spec_array = as_value.as_array().unwrap();

        let tool_call = &spec_array[0]["tool_calls"][0];
        assert_eq!(tool_call["thoughtSignature"], "sig_top_level");
        assert_eq!(
            tool_call["extra_content"]["google"]["thought_signature"],
            "sig_nested"
        );
        assert_eq!(tool_call["custom_field"], "custom_value");

        Ok(())
    }

    #[test]
    fn test_parallel_tool_responses_with_images_are_consecutive() -> anyhow::Result<()> {
        // Regression: #7449 — parallel tool calls with images must keep tool messages consecutive.
        let messages = vec![
            Message::assistant()
                .with_tool_request("id1", Ok(CallToolRequestParams::new("tool_a")))
                .with_tool_request("id2", Ok(CallToolRequestParams::new("tool_b"))),
            Message::user()
                .with_tool_response(
                    "id1",
                    Ok(CallToolResult::success(vec![ContentBlock::image(
                        "base64data1".to_string(),
                        "image/png".to_string(),
                    )])),
                )
                .with_tool_response(
                    "id2",
                    Ok(CallToolResult::success(vec![ContentBlock::image(
                        "base64data2".to_string(),
                        "image/png".to_string(),
                    )])),
                ),
        ];

        let as_value =
            serde_json::to_value(format_messages(&messages, &ImageFormat::OpenAi, None, true))
                .unwrap();
        let spec = as_value.as_array().unwrap();
        let roles: Vec<&str> = spec.iter().map(|m| m["role"].as_str().unwrap()).collect();

        // Without the fix this was ["assistant", "tool", "user", "tool", "user"].
        assert_eq!(roles, vec!["assistant", "tool", "tool", "user", "user"]);

        Ok(())
    }

    #[test]
    fn test_mixed_tool_responses_image_and_text_ordering() -> anyhow::Result<()> {
        // Mixed case: only one tool response has an image.
        let messages = vec![
            Message::assistant()
                .with_tool_request("id1", Ok(CallToolRequestParams::new("tool_a")))
                .with_tool_request("id2", Ok(CallToolRequestParams::new("tool_b"))),
            Message::user()
                .with_tool_response(
                    "id1",
                    Ok(CallToolResult::success(vec![ContentBlock::text(
                        "text result",
                    )])),
                )
                .with_tool_response(
                    "id2",
                    Ok(CallToolResult::success(vec![ContentBlock::image(
                        "base64data".to_string(),
                        "image/png".to_string(),
                    )])),
                ),
        ];

        let as_value =
            serde_json::to_value(format_messages(&messages, &ImageFormat::OpenAi, None, true))
                .unwrap();
        let spec = as_value.as_array().unwrap();
        let roles: Vec<&str> = spec.iter().map(|m| m["role"].as_str().unwrap()).collect();

        assert_eq!(roles, vec!["assistant", "tool", "tool", "user"]);

        Ok(())
    }
}
