//! Prompt-cache semantics declared per (provider, model), instead of implied
//! by whichever format module a request flows through.

use serde_json::{json, Value};

use crate::formats::openai::is_openai_responses_model;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheSemantics {
    /// Caller places markers; reuse needs an exact match of the marked bytes.
    ExplicitBreakpoints { max_breakpoints: usize },
    /// The longest matching stored prefix is reused implicitly.
    ImplicitTolerant,
    /// Only extends a prompt reproduced byte-for-byte from the start.
    ImplicitStrict,
    /// No known prompt cache.
    Uncached,
}

impl CacheSemantics {
    /// Unknown pairs default to `ImplicitStrict`, which is safe for every
    /// cache.
    pub fn for_model(provider_name: &str, model_name: &str) -> Self {
        match provider_name {
            "anthropic" | "minimax" | "zai" | "kimi_code" => {
                CacheSemantics::ExplicitBreakpoints { max_breakpoints: 4 }
            }
            "aws_bedrock" | "databricks" | "gcp_vertex_ai" if model_name.contains("claude") => {
                CacheSemantics::ExplicitBreakpoints { max_breakpoints: 4 }
            }
            "openrouter" | "litellm" if model_name.starts_with("anthropic/") => {
                CacheSemantics::ExplicitBreakpoints { max_breakpoints: 4 }
            }
            "openai" | "azure_openai" | "github_copilot" => {
                if is_openai_responses_model(model_name) {
                    CacheSemantics::ImplicitStrict
                } else {
                    CacheSemantics::ImplicitTolerant
                }
            }
            "moonshot" | "custom_deepseek" | "groq" | "together" | "fireworks-ai" | "mistral"
            | "zhipu" | "alibaba" => CacheSemantics::ImplicitTolerant,
            "snowflake" | "sagemaker_tgi" => CacheSemantics::Uncached,
            _ => CacheSemantics::ImplicitStrict,
        }
    }

    pub fn uses_explicit_breakpoints(self) -> bool {
        matches!(self, CacheSemantics::ExplicitBreakpoints { .. })
    }
}

/// Anthropic's cache lookback window, measured in content blocks: a new
/// breakpoint only finds a cached prefix within this distance of a prior one.
const LOOKBACK_BLOCKS: usize = 20;

/// Anthropic-dialect breakpoints for OpenAI-style chat payloads (OpenRouter,
/// LiteLLM, Databricks).
pub fn apply_chat_payload_breakpoints(payload: &mut Value) {
    if let Some(messages) = payload
        .get_mut("messages")
        .and_then(|messages| messages.as_array_mut())
    {
        // On this envelope tool results are `role: "tool"` and tool calls ride
        // on `role: "assistant"`, so anchoring by role would pin both message
        // breakpoints to the last human turn and re-bill the growing agentic
        // tail on every iteration.
        let offsets = cumulative_block_offsets(messages);
        if let Some(primary) = (0..messages.len())
            .rev()
            .find(|&index| has_cacheable_content(&messages[index]))
        {
            mark_last_content_block(&mut messages[primary]);
            // A trailing anchor ~LOOKBACK_BLOCKS behind keeps the next
            // request's tail breakpoint within the lookback window even when
            // one iteration appends many blocks (e.g. parallel tool calls).
            let target = offsets[primary].saturating_sub(LOOKBACK_BLOCKS);
            if let Some(secondary) = (0..primary)
                .rev()
                .find(|&index| offsets[index] <= target && has_cacheable_content(&messages[index]))
            {
                mark_last_content_block(&mut messages[secondary]);
            }
        }

        if let Some(system_message) = messages
            .iter_mut()
            .find(|message| message.get("role") == Some(&json!("system")))
        {
            mark_last_content_block(system_message);
        }
    }

    if let Some(last_tool) = payload
        .get_mut("tools")
        .and_then(|tools| tools.as_array_mut())
        .and_then(|tools| tools.last_mut())
    {
        if let Some(function) = last_tool.get_mut("function").and_then(Value::as_object_mut) {
            function.insert("cache_control".to_string(), json!({ "type": "ephemeral" }));
        }
    }
}

/// Maps each message index to the cumulative content-block count through the
/// end of that message. Each `tool_calls` entry becomes a `tool_use` content
/// block on the Anthropic side and retained `reasoning_content` a thinking
/// block, so they occupy lookback positions too.
fn cumulative_block_offsets(messages: &[Value]) -> Vec<usize> {
    let mut total = 0;
    messages
        .iter()
        .map(|message| {
            total += match message.get("content") {
                Some(Value::Array(blocks)) => blocks.len(),
                Some(Value::String(_)) => 1,
                _ => 0,
            };
            total += message
                .get("tool_calls")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            if message.get("reasoning_content").is_some() {
                total += 1;
            }
            total
        })
        .collect()
}

/// Whether a breakpoint can attach to the message's last content block.
/// Anthropic rejects `cache_control` on empty text (empty tool results
/// serialize to `content: ""`) and on thinking blocks (Databricks emits them
/// as `type: "reasoning"`); assistant tool-call messages carry their payload
/// in `tool_calls` with `content: null`.
fn has_cacheable_content(message: &Value) -> bool {
    match message.get("content") {
        Some(Value::String(text)) => !text.trim().is_empty(),
        Some(Value::Array(blocks)) => {
            blocks
                .last()
                .and_then(Value::as_object)
                .is_some_and(|block| match block.get("type").and_then(Value::as_str) {
                    Some("text") => block
                        .get("text")
                        .and_then(Value::as_str)
                        .is_some_and(|text| !text.trim().is_empty()),
                    Some("image_url") => true,
                    _ => false,
                })
        }
        _ => false,
    }
}

fn mark_last_content_block(message: &mut Value) {
    if !has_cacheable_content(message) {
        return;
    }
    let Some(content) = message.get_mut("content") else {
        return;
    };
    if let Some(text) = content.as_str() {
        *content = json!([{
            "type": "text",
            "text": text,
            "cache_control": { "type": "ephemeral" }
        }]);
        return;
    }
    if let Some(block) = content
        .as_array_mut()
        .and_then(|blocks| blocks.last_mut())
        .and_then(Value::as_object_mut)
    {
        block.insert("cache_control".to_string(), json!({ "type": "ephemeral" }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn for_model_resolves_registered_identifiers_and_defaults_to_strict() {
        let explicit = CacheSemantics::ExplicitBreakpoints { max_breakpoints: 4 };
        for (provider, model, expected) in [
            ("anthropic", "claude-sonnet-4-5", explicit),
            ("kimi_code", "kimi-for-coding", explicit),
            ("aws_bedrock", "anthropic.claude-sonnet-4-5", explicit),
            ("minimax", "MiniMax-M2.5", explicit),
            ("openrouter", "anthropic/claude-sonnet-4-5", explicit),
            (
                "openrouter",
                "openai/gpt-5.2",
                CacheSemantics::ImplicitStrict,
            ),
            ("openai", "gpt-5.2", CacheSemantics::ImplicitStrict),
            ("openai", "gpt-4.1-mini", CacheSemantics::ImplicitTolerant),
            ("snowflake", "llama-3.3-70b", CacheSemantics::Uncached),
            (
                "some_new_vendor",
                "some-model",
                CacheSemantics::ImplicitStrict,
            ),
        ] {
            assert_eq!(
                CacheSemantics::for_model(provider, model),
                expected,
                "{provider}/{model}"
            );
        }
    }

    fn tool_heavy_messages(iterations: usize) -> Vec<Value> {
        let mut messages = vec![json!({"role": "user", "content": "start"})];
        for i in 0..iterations {
            messages.push(json!({
                "role": "assistant",
                "content": null,
                "reasoning_content": format!("thinking {i}"),
                "tool_calls": [{
                    "id": format!("call_{i}"),
                    "type": "function",
                    "function": {"name": "read_file", "arguments": "{}"}
                }]
            }));
            messages.push(json!({
                "role": "tool",
                "content": format!("result {i}"),
                "tool_call_id": format!("call_{i}")
            }));
        }
        messages
    }

    fn marked_message_indices(payload: &Value) -> Vec<usize> {
        payload["messages"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
            .filter(|(_, message)| {
                message["content"].as_array().is_some_and(|blocks| {
                    blocks
                        .iter()
                        .any(|block| block.get("cache_control").is_some())
                })
            })
            .map(|(index, _)| index)
            .collect()
    }

    #[test]
    fn breakpoints_cover_system_tools_and_the_tail_message() {
        let mut payload = json!({
            "model": "anthropic/claude-sonnet-4-5",
            "messages": [
                {"role": "system", "content": "be careful"},
                {"role": "user", "content": "first"},
                {"role": "assistant", "content": "ok"},
                {"role": "user", "content": "second"},
                {"role": "assistant", "content": "ok"},
                {"role": "user", "content": "third"}
            ],
            "tools": [
                {"type": "function", "function": {"name": "a"}},
                {"type": "function", "function": {"name": "b"}}
            ]
        });
        apply_chat_payload_breakpoints(&mut payload);

        let messages = payload["messages"].as_array().unwrap();
        assert_eq!(
            messages[0]["content"][0]["cache_control"]["type"],
            "ephemeral"
        );
        assert!(messages[1]["content"].is_string());
        assert!(messages[3]["content"].is_string());
        assert_eq!(
            messages[5]["content"][0]["cache_control"]["type"],
            "ephemeral"
        );
        let tools = payload["tools"].as_array().unwrap();
        assert!(tools[0]["function"].get("cache_control").is_none());
        assert_eq!(tools[1]["function"]["cache_control"]["type"], "ephemeral");
    }

    #[test]
    fn unmarkable_tail_falls_back_to_the_last_markable_message() {
        let mut payload = json!({
            "messages": [
                {"role": "tool", "content": "result", "tool_call_id": "call_0"},
                {"role": "assistant", "content": null, "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": {"name": "noop", "arguments": "{}"}
                }]},
                {"role": "tool", "content": "", "tool_call_id": "call_1"}
            ]
        });
        apply_chat_payload_breakpoints(&mut payload);

        let messages = payload["messages"].as_array().unwrap();
        assert_eq!(messages[2]["content"], json!(""));
        assert_eq!(marked_message_indices(&payload), vec![0]);
    }

    #[test]
    fn secondary_anchor_trails_the_tail_and_total_stays_within_budget() {
        let mut messages = vec![json!({"role": "system", "content": "be careful"})];
        messages.extend(tool_heavy_messages(25));
        let mut payload = json!({
            "messages": messages,
            "tools": [{"type": "function", "function": {"name": "read_file"}}]
        });
        apply_chat_payload_breakpoints(&mut payload);

        // 77 cumulative blocks (each iteration is one thinking block, one
        // tool_use, and one tool result); the trailing anchor must sit on the
        // last message at or below 77 - LOOKBACK_BLOCKS = 57 cumulative
        // blocks: the eighteenth tool result, at message index 37.
        assert_eq!(marked_message_indices(&payload), vec![0, 37, 51]);
        assert_eq!(payload.to_string().matches("cache_control").count(), 4);
    }

    #[test]
    fn array_content_user_messages_get_a_breakpoint() {
        let mut payload = json!({
            "messages": [
                {"role": "user", "content": [
                    {"type": "text", "text": "look at this"},
                    {"type": "image_url", "image_url": {"url": "data:image/png;base64,xyz"}}
                ]}
            ]
        });
        apply_chat_payload_breakpoints(&mut payload);
        let blocks = payload["messages"][0]["content"].as_array().unwrap();
        assert!(blocks[0].get("cache_control").is_none());
        assert_eq!(blocks[1]["cache_control"]["type"], "ephemeral");
    }
}
