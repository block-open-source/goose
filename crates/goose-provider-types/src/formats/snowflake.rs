use crate::conversation::message::{Message, MessageContentBlock};
use crate::conversation::token_usage::Usage;
use crate::documents::{unsupported_document_text, UNSUPPORTED_PROVIDER_REASON};
use crate::errors::ProviderError;
use crate::mcp_utils::extract_text_from_resource;
use crate::model::ModelConfig;
use anyhow::{anyhow, Result};
use rmcp::model::{object, CallToolRequestParams, Role, Tool};
use rmcp::object;
use serde_json::{json, Value};
use std::collections::HashSet;

/// Convert internal Message format to Snowflake's API message specification
pub fn format_messages(messages: &[Message]) -> Vec<Value> {
    let mut snowflake_messages = Vec::new();

    for message in messages {
        let role = match message.role {
            Role::User => "user",
            Role::Assistant => "assistant",
        };

        let mut text_content = String::new();

        for msg_content in &message.content {
            match msg_content {
                MessageContentBlock::Text(text) => {
                    if !text_content.is_empty() {
                        text_content.push('\n');
                    }
                    text_content.push_str(&text.text);
                }
                MessageContentBlock::ToolRequest(_tool_request) => {
                    // Skip tool requests in message formatting - tools are handled separately
                    // through the tools parameter in the API request
                    continue;
                }
                MessageContentBlock::ToolResponse(tool_response) => {
                    if let Ok(result) = &tool_response.tool_result {
                        let text = result
                            .content
                            .iter()
                            .filter_map(|c| {
                                if let Some(t) = c.as_text() {
                                    return Some(t.text.clone());
                                }
                                if let Some(r) = c.as_resource() {
                                    let text = extract_text_from_resource(&r.resource);
                                    if !text.is_empty() {
                                        return Some(text);
                                    }
                                }
                                None
                            })
                            .collect::<Vec<_>>()
                            .join("\n");

                        if !text_content.is_empty() {
                            text_content.push('\n');
                        }
                        if !text.is_empty() {
                            text_content.push_str(&format!("Tool result: {}", text));
                        }
                    }
                }
                MessageContentBlock::ToolConfirmationRequest(_) => {}
                MessageContentBlock::ActionRequired(_) => {}
                MessageContentBlock::SystemNotification(_) | MessageContentBlock::Error(_) => {
                    // Skip
                }
                MessageContentBlock::Thinking(_thinking) => {
                    // Skip thinking for now
                }
                MessageContentBlock::RedactedThinking(_redacted) => {
                    // Skip redacted thinking for now
                }
                MessageContentBlock::Image(_) => continue, // Snowflake doesn't support image content yet
                MessageContentBlock::Document(document) => {
                    if !text_content.is_empty() {
                        text_content.push('\n');
                    }
                    text_content.push_str(&unsupported_document_text(
                        document,
                        UNSUPPORTED_PROVIDER_REASON,
                    ));
                }
            }
        }

        // Add message if it has text content
        if !text_content.is_empty() {
            snowflake_messages.push(json!({
                "role": role,
                "content": text_content
            }));
        }
    }

    // Only add default message if we truly have no messages at all
    // This should be rare and only for edge cases
    if snowflake_messages.is_empty() {
        snowflake_messages.push(json!({
            "role": "user",
            "content": "Continue the conversation"
        }));
    }

    snowflake_messages
}

/// Convert internal Tool format to Snowflake's API tool specification
pub fn format_tools(tools: &[Tool]) -> Vec<Value> {
    let mut unique_tools = HashSet::new();
    let mut tool_specs = Vec::new();

    for tool in tools.iter() {
        if unique_tools.insert(tool.name.clone()) {
            let tool_spec = json!({
                "type": "generic",
                "name": tool.name,
                "description": tool.description,
                "input_schema": tool.input_schema
            });

            tool_specs.push(json!({"tool_spec": tool_spec}));
        }
    }

    tool_specs
}

/// Convert system message to Snowflake's API system specification
pub fn format_system(system: &str) -> Value {
    json!({
        "role": "system",
        "content": system,
    })
}

/// Convert Snowflake's streaming API response to internal Message format
pub fn parse_streaming_response(sse_data: &str) -> Result<Message> {
    let mut message = Message::assistant();
    let mut accumulated_text = String::new();
    let mut tool_use_id: Option<String> = None;
    let mut tool_name: Option<String> = None;
    let mut tool_input = String::new();

    // Parse each SSE event
    for line in sse_data.lines() {
        // SSE spec allows both "data: value" and "data:value" (space after colon is optional)
        if !line.starts_with("data:") {
            continue;
        }

        let json_str = line
            .strip_prefix("data: ")
            .or_else(|| line.strip_prefix("data:"))
            .unwrap(); // Remove "data:" prefix
        if json_str.trim().is_empty() || json_str.trim() == "[DONE]" {
            continue;
        }

        let event: Value = match serde_json::from_str(json_str) {
            Ok(v) => v,
            Err(_) => {
                continue;
            }
        };

        if let Some(choices) = event.get("choices").and_then(|c| c.as_array()) {
            if let Some(choice) = choices.first() {
                if let Some(delta) = choice.get("delta") {
                    match delta.get("type").and_then(|t| t.as_str()) {
                        Some("text") => {
                            if let Some(content) = delta.get("content").and_then(|c| c.as_str()) {
                                accumulated_text.push_str(content);
                            }
                        }
                        Some("tool_use") => {
                            if let Some(id) = delta.get("tool_use_id").and_then(|i| i.as_str()) {
                                tool_use_id = Some(id.to_string());
                            }
                            if let Some(name) = delta.get("name").and_then(|n| n.as_str()) {
                                tool_name = Some(name.to_string());
                            }
                            if let Some(input) = delta.get("input").and_then(|i| i.as_str()) {
                                tool_input.push_str(input);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    // Add accumulated text if any
    if !accumulated_text.is_empty() {
        message = message.with_text(accumulated_text);
    }

    // Add tool use if complete
    if let Some((id, name)) = tool_use_id.zip(tool_name) {
        if !tool_input.is_empty() {
            let input_value = serde_json::from_str::<Value>(&tool_input)
                .unwrap_or_else(|_| Value::String(tool_input.clone()));
            let tool_call = CallToolRequestParams::new(name).with_arguments(object(input_value));
            message = message.with_tool_request(&id, Ok(tool_call));
        } else {
            // Tool with no input - use empty object
            let tool_call = CallToolRequestParams::new(name).with_arguments(object!({}));
            message = message.with_tool_request(&id, Ok(tool_call));
        }
    }

    Ok(message)
}

/// Convert Snowflake's API response to internal Message format
pub fn response_to_message(response: &Value) -> Result<Message> {
    let mut message = Message::assistant();

    let content_list = response.get("content_list").and_then(|cl| cl.as_array());

    // Handle case where content_list is missing or empty
    let content_list = match content_list {
        Some(list) if !list.is_empty() => list,
        _ => {
            // If no content_list or empty, check if there's a direct content field
            if let Some(direct_content) = response.get("content").and_then(|c| c.as_str()) {
                if !direct_content.is_empty() {
                    message = message.with_text(direct_content.to_string());
                }
                return Ok(message);
            } else {
                // Return empty assistant message for empty responses
                return Ok(message);
            }
        }
    };

    // Process all content items in the list
    for content in content_list {
        match content.get("type").and_then(|t| t.as_str()) {
            Some("text") => {
                if let Some(text) = content.get("text").and_then(|t| t.as_str()) {
                    if !text.is_empty() {
                        message = message.with_text(text.to_string());
                    }
                }
            }
            Some("tool_use") => {
                let id = content
                    .get("tool_use_id")
                    .and_then(|i| i.as_str())
                    .ok_or_else(|| anyhow!("Missing tool_use id"))?;
                let name = content
                    .get("name")
                    .and_then(|n| n.as_str())
                    .ok_or_else(|| anyhow!("Missing tool_use name"))?
                    .to_string();

                let input = content
                    .get("input")
                    .ok_or_else(|| anyhow!("Missing tool input"))?
                    .clone();

                let tool_call = CallToolRequestParams::new(name).with_arguments(object(input));
                message = message.with_tool_request(id, Ok(tool_call));
            }
            Some("thinking") => {
                let thinking = content
                    .get("thinking")
                    .and_then(|t| t.as_str())
                    .ok_or_else(|| anyhow!("Missing thinking content"))?;
                let signature = content
                    .get("signature")
                    .and_then(|s| s.as_str())
                    .ok_or_else(|| anyhow!("Missing thinking signature"))?;
                message = message.with_thinking(thinking, signature);
            }
            Some("redacted_thinking") => {
                let data = content
                    .get("data")
                    .and_then(|d| d.as_str())
                    .ok_or_else(|| anyhow!("Missing redacted_thinking data"))?;
                message = message.with_redacted_thinking(data);
            }
            _ => {
                // Ignore unrecognized content types
            }
        }
    }

    Ok(message)
}

/// Extract usage information from Snowflake's API response
pub fn get_usage(data: &Value) -> Result<Usage> {
    // Extract usage data if available
    if let Some(usage) = data.get("usage") {
        let input_tokens = usage
            .get("input_tokens")
            .and_then(|v| v.as_u64())
            .map(|v| v as i32);

        let output_tokens = usage
            .get("output_tokens")
            .and_then(|v| v.as_u64())
            .map(|v| v as i32);

        let total_tokens = match (input_tokens, output_tokens) {
            (Some(input), Some(output)) => Some(input + output),
            _ => None,
        };

        Ok(Usage::new(input_tokens, output_tokens, total_tokens))
    } else {
        tracing::debug!(
            "Failed to get usage data: {}",
            ProviderError::UsageError("No usage data found in response".to_string())
        );
        // If no usage data, return None for all values
        Ok(Usage::new(None, None, None))
    }
}

/// Create a complete request payload for Snowflake's API
pub fn create_request(
    model_config: &ModelConfig,
    system: &str,
    messages: &[Message],
    tools: &[Tool],
) -> Result<Value> {
    let mut snowflake_messages = format_messages(messages);
    let system_spec = format_system(system);

    // Add system message to the beginning of the messages
    snowflake_messages.insert(0, system_spec);

    // Check if we have any messages to send
    if snowflake_messages.is_empty() {
        return Err(anyhow!("No valid messages to send to Snowflake API"));
    }

    // Detect description generation requests and exclude tools to prevent interference
    // with normal tool execution flow
    let is_description_request =
        system.contains("Reply with only a description in four words or less");

    let tool_specs = if is_description_request {
        // For description generation, don't include any tools to avoid confusion
        format_tools(&[])
    } else {
        format_tools(tools)
    };

    let mut payload = json!({
        "model": model_config.model_name,
        "messages": snowflake_messages,
        "max_tokens": model_config.max_output_tokens(),
    });

    // Add tools if present and not a description request
    if !tool_specs.is_empty() {
        if let Some(obj) = payload.as_object_mut() {
            obj.insert("tools".to_string(), json!(tool_specs));
        } else {
            return Err(anyhow!(
                "Failed to create request payload: payload is not a JSON object"
            ));
        }
    }

    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::message::Message;
    use rmcp::object;
    use serde_json::json;

    #[test]
    fn test_parse_text_response() -> Result<()> {
        let response = json!({
            "id": "msg_123",
            "type": "message",
            "role": "assistant",
            "content_list": [{
                "type": "text",
                "text": "Hello! How can I assist you today?"
            }],
            "model": "claude-4-sonnet",
            "stop_reason": "end_turn",
            "stop_sequence": null,
            "usage": {
                "input_tokens": 12,
                "output_tokens": 15
            }
        });

        let message = response_to_message(&response)?;
        let usage = get_usage(&response)?;

        if let MessageContentBlock::Text(text) = &message.content[0] {
            assert_eq!(text.text, "Hello! How can I assist you today?");
        } else {
            panic!("Expected Text content");
        }

        assert_eq!(usage.input_tokens, Some(12));
        assert_eq!(usage.output_tokens, Some(15));
        assert_eq!(usage.total_tokens, Some(27)); // 12 + 15

        Ok(())
    }

    #[test]
    fn test_parse_tool_response() -> Result<()> {
        let response = json!({
            "id": "msg_123",
            "type": "message",
            "role": "assistant",
            "content_list": [{
                "type": "tool_use",
                "tool_use_id": "tool_1",
                "name": "calculator",
                "input": {"expression": "2 + 2"}
            }],
            "model": "claude-4-sonnet",
            "stop_reason": "end_turn",
            "stop_sequence": null,
            "usage": {
                "input_tokens": 15,
                "output_tokens": 20
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

        assert_eq!(usage.input_tokens, Some(15));
        assert_eq!(usage.output_tokens, Some(20));
        assert_eq!(usage.total_tokens, Some(35)); // 15 + 20

        Ok(())
    }

    #[test]
    fn test_message_to_snowflake_spec() {
        let messages = vec![
            Message::user().with_text("Hello"),
            Message::assistant().with_text("Hi there"),
            Message::user().with_text("How are you?"),
        ];

        let spec = format_messages(&messages);

        assert_eq!(spec.len(), 3);
        assert_eq!(spec[0]["role"], "user");
        assert_eq!(spec[0]["content"], "Hello");
        assert_eq!(spec[1]["role"], "assistant");
        assert_eq!(spec[1]["content"], "Hi there");
        assert_eq!(spec[2]["role"], "user");
        assert_eq!(spec[2]["content"], "How are you?");
    }

    #[test]
    fn test_tools_to_snowflake_spec() {
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

        let spec = format_tools(&tools);

        assert_eq!(spec.len(), 2);
        assert_eq!(spec[0]["tool_spec"]["name"], "calculator");
        assert_eq!(
            spec[0]["tool_spec"]["description"],
            "Calculate mathematical expressions"
        );
        assert_eq!(spec[1]["tool_spec"]["name"], "weather");
        assert_eq!(
            spec[1]["tool_spec"]["description"],
            "Get weather information"
        );
    }

    #[test]
    fn test_system_to_snowflake_spec() {
        let system = "You are a helpful assistant.";
        let spec = format_system(system);

        assert_eq!(spec["role"], "system");
        assert_eq!(spec["content"], system);
    }

    #[test]
    fn test_parse_streaming_response() -> Result<()> {
        let sse_data = r#"data: {"id":"a9537c2c-2017-4906-9817-2456168d89fa","model":"claude-sonnet-4-20250514","choices":[{"delta":{"type":"text","content":"I","content_list":[{"type":"text","text":"I"}],"text":"I"}}],"usage":{}}

data: {"id":"a9537c2c-2017-4906-9817-2456168d89fa","model":"claude-sonnet-4-20250514","choices":[{"delta":{"type":"text","content":"'ll help you check Nvidia's current","content_list":[{"type":"text","text":"'ll help you check Nvidia's current"}],"text":"'ll help you check Nvidia's current"}}],"usage":{}}

data: {"id":"a9537c2c-2017-4906-9817-2456168d89fa","model":"claude-sonnet-4-20250514","choices":[{"delta":{"type":"tool_use","tool_use_id":"tooluse_FB_nOElDTAOKa-YnVWI5Uw","name":"get_stock_price","content_list":[{"tool_use_id":"tooluse_FB_nOElDTAOKa-YnVWI5Uw","name":"get_stock_price"}],"text":""}}],"usage":{}}

data: {"id":"a9537c2c-2017-4906-9817-2456168d89fa","model":"claude-sonnet-4-20250514","choices":[{"delta":{"type":"tool_use","input":"{\"symbol\":\"NVDA\"}","content_list":[{"input":"{\"symbol\":\"NVDA\"}"}],"text":""}}],"usage":{"prompt_tokens":397,"completion_tokens":65,"total_tokens":462}}
"#;

        let message = parse_streaming_response(sse_data)?;

        // Should have both text and tool request
        assert_eq!(message.content.len(), 2);

        if let MessageContentBlock::Text(text) = &message.content[0] {
            assert!(text.text.contains("I'll help you check Nvidia's current"));
        } else {
            panic!("Expected Text content first");
        }

        if let MessageContentBlock::ToolRequest(tool_request) = &message.content[1] {
            let tool_call = tool_request.tool_call.as_ref().unwrap();
            assert_eq!(tool_call.name, "get_stock_price");
            assert_eq!(tool_call.arguments, Some(object!({"symbol": "NVDA"})));
            assert_eq!(tool_request.id, "tooluse_FB_nOElDTAOKa-YnVWI5Uw");
        } else {
            panic!("Expected ToolRequest content second");
        }

        Ok(())
    }

    #[test]
    fn test_create_request_format() -> Result<()> {
        use crate::conversation::message::Message;
        use crate::model::ModelConfig;

        let model_config = ModelConfig::new("claude-4-sonnet").with_canonical_limits("snowflake");

        let system = "You are a helpful assistant that can use tools to get information.";
        let messages = vec![Message::user().with_text("What is the stock price of Nvidia?")];

        let tools = vec![Tool::new(
            "get_stock_price",
            "Get stock price information",
            object!({
                "type": "object",
                "properties": {
                    "symbol": {
                        "type": "string",
                        "description": "The symbol for the stock ticker, e.g. Snowflake = SNOW"
                    }
                },
                "required": ["symbol"]
            }),
        )];

        let request = create_request(&model_config, system, &messages, &tools)?;

        // Check basic structure
        assert_eq!(request["model"], "claude-4-sonnet");

        let messages_array = request["messages"].as_array().unwrap();
        assert_eq!(messages_array.len(), 2); // system + user message

        // First message should be system with simple content
        assert_eq!(messages_array[0]["role"], "system");
        assert_eq!(
            messages_array[0]["content"],
            "You are a helpful assistant that can use tools to get information."
        );

        // Second message should be user with simple content
        assert_eq!(messages_array[1]["role"], "user");
        assert_eq!(
            messages_array[1]["content"],
            "What is the stock price of Nvidia?"
        );

        // Tools should have tool_spec wrapper
        let tools_array = request["tools"].as_array().unwrap();
        assert_eq!(tools_array[0]["tool_spec"]["name"], "get_stock_price");

        Ok(())
    }

    #[test]
    fn test_parse_mixed_text_and_tool_response() -> Result<()> {
        let response = json!({
            "id": "msg_123",
            "type": "message",
            "role": "assistant",
            "content": "I'll help you with that calculation.",
            "content_list": [
                {
                    "type": "text",
                    "text": "I'll help you with that calculation."
                },
                {
                    "type": "tool_use",
                    "tool_use_id": "tool_1",
                    "name": "calculator",
                    "input": {"expression": "2 + 2"}
                }
            ],
            "model": "claude-4-sonnet",
            "usage": {
                "input_tokens": 10,
                "output_tokens": 15
            }
        });

        let message = response_to_message(&response)?;

        // Should have both text and tool request content
        assert_eq!(message.content.len(), 2);

        if let MessageContentBlock::Text(text) = &message.content[0] {
            assert_eq!(text.text, "I'll help you with that calculation.");
        } else {
            panic!("Expected Text content first");
        }

        if let MessageContentBlock::ToolRequest(tool_request) = &message.content[1] {
            let tool_call = tool_request.tool_call.as_ref().unwrap();
            assert_eq!(tool_call.name, "calculator");
            assert_eq!(tool_request.id, "tool_1");
        } else {
            panic!("Expected ToolRequest content second");
        }

        Ok(())
    }

    #[test]
    fn test_empty_tools_array() {
        let tools: Vec<Tool> = vec![];
        let spec = format_tools(&tools);
        assert_eq!(spec.len(), 0);
    }

    #[test]
    fn test_create_request_excludes_tools_for_description() -> Result<()> {
        use crate::conversation::message::Message;
        use crate::model::ModelConfig;

        let model_config = ModelConfig::new("claude-4-sonnet").with_canonical_limits("snowflake");
        let system = "Reply with only a description in four words or less";
        let messages = vec![Message::user().with_text("Test message")];
        let tools = vec![Tool::new(
            "test_tool",
            "Test tool",
            object!({"type": "object", "properties": {}}),
        )];

        let request = create_request(&model_config, system, &messages, &tools)?;

        // Should not include tools for description requests
        assert!(request.get("tools").is_none());

        Ok(())
    }

    #[test]
    fn test_message_formatting_skips_tool_requests() {
        use crate::conversation::message::Message;

        // Create a conversation with text, tool requests, and tool responses
        let tool_call = CallToolRequestParams::new("calculator")
            .with_arguments(object!({"expression": "2 + 2"}));

        let messages = vec![
            Message::user().with_text("Calculate 2 + 2"),
            Message::assistant()
                .with_text("I'll help you calculate that.")
                .with_tool_request("tool_1", Ok(tool_call)),
            Message::user().with_text("Thanks!"),
        ];

        let spec = format_messages(&messages);

        // Should only have 3 messages - the tool request should be skipped
        assert_eq!(spec.len(), 3);
        assert_eq!(spec[0]["role"], "user");
        assert_eq!(spec[0]["content"], "Calculate 2 + 2");
        assert_eq!(spec[1]["role"], "assistant");
        assert_eq!(spec[1]["content"], "I'll help you calculate that.");
        assert_eq!(spec[2]["role"], "user");
        assert_eq!(spec[2]["content"], "Thanks!");

        // Verify no tool request content is in the message history
        for message in &spec {
            let content = message["content"].as_str().unwrap();
            assert!(!content.contains("Using tool:"));
            assert!(!content.contains("calculator"));
        }
    }
}
