//! # ToolShim Module
//!
//! The ToolShim module provides a reusable component for interpreting and augmenting LLM outputs with tool calls,
//! regardless of whether the underlying model natively supports tool/function calling.
//!
//! ## Overview
//!
//! ToolShim addresses the challenge of working with models that don't natively support tools by:
//!
//! 1. Taking the text output from any LLM
//! 2. Sending it to a separate "interpreter" model (which can be the same or different model)
//! 3. Using a model to extract tool call intentions into the appropriate format
//! 4. Converting the outputs of the interpreter model into proper tool call structs
//! 5. Augmenting the original message with the extracted tool calls
//!
//! ## Key Components
//!
//! ### ToolInterpreter Trait
//!
//! The core of ToolShim is the `ToolInterpreter` trait, which defines the interface for any model that can interpret text and extract tool calls.
//!
//! ### Implementations
//!
//! The module provides an implementation for Ollama:
//!
//! - `OllamaInterpreter`: Uses Ollama's structured output API to interpret tool calls
//!
//! ### Helper Functions
//!
//! - `augment_message_with_tool_calls`: A utility function that takes any message, extracts text content, sends it to an interpreter, and adds any detected tool calls back to the message.
//!

#[cfg(feature = "local-inference")]
use super::local_inference::LOCAL_LLM_MODEL_CONFIG_KEY;
use super::ollama::OLLAMA_DEFAULT_PORT;
use super::ollama::OLLAMA_HOST;
use crate::conversation::message::{Message, MessageContent};
use crate::conversation::Conversation;
use crate::model_config::model_config_from_user_config;
use crate::providers::base::DEFAULT_PROVIDER_TIMEOUT_SECS;
use anyhow::Result;
use futures::StreamExt;
use goose_providers::errors::ProviderError;
use goose_providers::formats::openai::create_request;
use goose_providers::images::ImageFormat;
use reqwest::Client;
use rmcp::model::{object, CallToolRequestParams, ContentBlock, Tool};
use serde::de::{DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::{json, Value};
use std::fmt;
use std::time::Duration;
use uuid::Uuid;

/// Default model to use for tool interpretation
pub const DEFAULT_INTERPRETER_MODEL_OLLAMA: &str = "mistral-nemo";
pub const TOOLSHIM_BACKEND_ENV_VAR: &str = "GOOSE_TOOLSHIM_BACKEND";
pub const TOOLSHIM_LOCAL_MODEL_ENV_VAR: &str = "GOOSE_TOOLSHIM_MODEL";
#[cfg(not(feature = "local-inference"))]
const LOCAL_LLM_MODEL_CONFIG_KEY: &str = "LOCAL_LLM_MODEL";

const TOOL_CALLS_SECTION_BEGIN: &str = "<|tool_calls_section_begin|>";
const TOOL_CALLS_SECTION_END: &str = "<|tool_calls_section_end|>";
const TOOL_CALL_BEGIN: &str = "<|tool_call_begin|>";
const TOOL_CALL_ARGUMENT_BEGIN: &str = "<|tool_call_argument_begin|>";
const TOOL_CALL_ARGUMENT_END: &str = "<|tool_call_argument_end|>";
const TOOL_CALL_END: &str = "<|tool_call_end|>";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolshimBackend {
    Ollama,
    Local,
}

fn parse_toolshim_backend(value: &str) -> Result<ToolshimBackend, ProviderError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "ollama" => Ok(ToolshimBackend::Ollama),
        "local" | "llama.cpp" | "llama_cpp" => Ok(ToolshimBackend::Local),
        other => Err(ProviderError::RequestFailed(format!(
            "Invalid {} value '{}'. Expected one of: ollama, local, llama.cpp",
            TOOLSHIM_BACKEND_ENV_VAR, other
        ))),
    }
}

fn get_toolshim_backend() -> Result<ToolshimBackend, ProviderError> {
    match std::env::var(TOOLSHIM_BACKEND_ENV_VAR) {
        Ok(value) => parse_toolshim_backend(&value),
        Err(_) => Ok(ToolshimBackend::Ollama),
    }
}

fn resolve_local_interpreter_model() -> Result<String, ProviderError> {
    let env_model = std::env::var(TOOLSHIM_LOCAL_MODEL_ENV_VAR)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let config_model = crate::config::Config::global()
        .get_param::<String>(LOCAL_LLM_MODEL_CONFIG_KEY)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    resolve_local_interpreter_model_from_sources(env_model, config_model)
}

fn resolve_local_interpreter_model_from_sources(
    env_model: Option<String>,
    config_model: Option<String>,
) -> Result<String, ProviderError> {
    env_model.or(config_model).ok_or_else(|| {
        ProviderError::RequestFailed(format!(
            "Local toolshim backend requires {} or {} to be set",
            TOOLSHIM_LOCAL_MODEL_ENV_VAR, LOCAL_LLM_MODEL_CONFIG_KEY
        ))
    })
}

fn resolve_tool_name(raw_tool_name: &str, tools: &[Tool]) -> Option<String> {
    let trimmed = raw_tool_name.trim();
    let without_index = trimmed.split(':').next().unwrap_or(trimmed).trim();
    let without_functions_prefix = without_index
        .strip_prefix("functions.")
        .unwrap_or(without_index)
        .trim();
    let short_name = without_functions_prefix
        .rsplit('.')
        .next()
        .unwrap_or(without_functions_prefix)
        .trim();

    // Also try replacing dots with double-underscores (goose tool name convention)
    let with_dunder = without_functions_prefix.replace('.', "__");

    let mut candidates = vec![
        trimmed.to_string(),
        without_index.to_string(),
        without_functions_prefix.to_string(),
        with_dunder,
        short_name.to_string(),
    ];
    candidates.dedup();

    for candidate in &candidates {
        if tools.iter().any(|tool| tool.name == *candidate) {
            return Some(candidate.clone());
        }
    }

    for candidate in &candidates {
        let mut matches: Vec<String> = tools
            .iter()
            .filter(|tool| tool.name.ends_with(&format!("__{}", candidate)))
            .map(|tool| tool.name.to_string())
            .collect();
        matches.sort();
        matches.dedup();

        if matches.len() == 1 {
            return Some(matches[0].clone());
        }
    }

    None
}

fn normalized_tool_alias(raw_tool_name: &str) -> String {
    let trimmed = raw_tool_name.trim();
    let without_index = trimmed.split(':').next().unwrap_or(trimmed).trim();
    let without_functions_prefix = without_index
        .strip_prefix("functions.")
        .unwrap_or(without_index)
        .trim();

    without_functions_prefix
        .rsplit('.')
        .next()
        .unwrap_or(without_functions_prefix)
        .trim()
        .trim_matches(|character: char| !character.is_ascii_alphanumeric() && character != '_')
        .to_ascii_lowercase()
}

fn contains_unresolved_execute_alias(raw_tool_header: &str, tools: &[Tool]) -> bool {
    raw_tool_header.split_whitespace().any(|raw_tool_name| {
        raw_tool_name.split(':').any(|segment| {
            matches!(
                normalized_tool_alias(segment).as_str(),
                "execute" | "execute_code"
            ) && resolve_tool_name(segment, tools).is_none()
        })
    })
}

fn contains_structured_unresolved_execute_alias(value: &Value, tools: &[Tool]) -> bool {
    match value {
        Value::Object(object) => {
            object
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(|name| contains_unresolved_execute_alias(name, tools))
                || object
                    .values()
                    .any(|value| contains_structured_unresolved_execute_alias(value, tools))
        }
        Value::Array(array) => array
            .iter()
            .any(|value| contains_structured_unresolved_execute_alias(value, tools)),
        _ => false,
    }
}

struct RawStructuredExecuteAliasSeed<'a> {
    tools: &'a [Tool],
    inspect_string: bool,
}

struct RawStructuredExecuteAliasVisitor<'a> {
    tools: &'a [Tool],
    inspect_string: bool,
}

impl<'de> DeserializeSeed<'de> for RawStructuredExecuteAliasSeed<'_> {
    type Value = bool;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(RawStructuredExecuteAliasVisitor {
            tools: self.tools,
            inspect_string: self.inspect_string,
        })
    }
}

impl<'de> Visitor<'de> for RawStructuredExecuteAliasVisitor<'_> {
    type Value = bool;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value")
    }

    fn visit_bool<E>(self, _value: bool) -> Result<Self::Value, E> {
        Ok(false)
    }

    fn visit_i64<E>(self, _value: i64) -> Result<Self::Value, E> {
        Ok(false)
    }

    fn visit_u64<E>(self, _value: u64) -> Result<Self::Value, E> {
        Ok(false)
    }

    fn visit_f64<E>(self, _value: f64) -> Result<Self::Value, E> {
        Ok(false)
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
        Ok(self.inspect_string && contains_unresolved_execute_alias(value, self.tools))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(self.inspect_string && contains_unresolved_execute_alias(&value, self.tools))
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(false)
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut contains_execute = false;
        while let Some(value_contains_execute) =
            sequence.next_element_seed(RawStructuredExecuteAliasSeed {
                tools: self.tools,
                inspect_string: false,
            })?
        {
            contains_execute |= value_contains_execute;
        }
        Ok(contains_execute)
    }

    fn visit_map<A>(self, mut object: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut contains_execute = false;
        while let Some(key) = object.next_key::<String>()? {
            let value_contains_execute = object.next_value_seed(RawStructuredExecuteAliasSeed {
                tools: self.tools,
                inspect_string: key == "name",
            })?;
            contains_execute |= value_contains_execute;
        }
        Ok(contains_execute)
    }
}

fn raw_arguments_contain_structured_unresolved_execute_alias(
    raw_arguments: &str,
    tools: &[Tool],
) -> bool {
    let mut deserializer = serde_json::Deserializer::from_str(raw_arguments);
    RawStructuredExecuteAliasSeed {
        tools,
        inspect_string: false,
    }
    .deserialize(&mut deserializer)
    .unwrap_or(false)
}

fn malformed_arguments_contain_unresolved_execute_alias(
    raw_arguments: &str,
    tools: &[Tool],
) -> bool {
    let mut remainder = raw_arguments.trim();
    while !remainder.is_empty() {
        let mut values = serde_json::Deserializer::from_str(remainder).into_iter::<Value>();
        if let Some(Ok(value)) = values.next() {
            if contains_structured_unresolved_execute_alias(&value, tools) {
                return true;
            }
            remainder = remainder
                .get(values.byte_offset()..)
                .unwrap_or_default()
                .trim_start();
            continue;
        }

        let prefix_end = remainder.find('{').unwrap_or(remainder.len());
        let (non_json_prefix, json_suffix) = remainder.split_at(prefix_end);
        if contains_unresolved_execute_alias(non_json_prefix, tools) {
            return true;
        }
        if prefix_end == 0 {
            remainder = remainder.strip_prefix('{').unwrap_or_default().trim_start();
            continue;
        }
        if prefix_end == remainder.len() {
            return false;
        }
        remainder = json_suffix;
    }
    false
}

fn decode_quoted_string(literal: &str) -> Option<String> {
    let quote = literal.chars().next()?;
    if !matches!(quote, '"' | '\'') || !literal.ends_with(quote) {
        return None;
    }

    let body = literal.strip_prefix(quote)?.strip_suffix(quote)?;
    let mut escaped = false;
    let mut decoded = String::new();
    for ch in body.chars() {
        if escaped {
            decoded.push(ch);
            escaped = false;
            continue;
        }

        if ch == '\\' {
            escaped = true;
            continue;
        }

        decoded.push(ch);
    }

    (!escaped).then_some(decoded)
}

fn node_text<'a>(node: tree_sitter::Node<'_>, source: &'a str) -> Option<&'a str> {
    source.get(node.byte_range())
}

fn is_developer_shell_call(node: tree_sitter::Node<'_>, source: &str) -> bool {
    let Some(function) = node.child_by_field_name("function") else {
        return false;
    };
    if function.kind() != "member_expression" {
        return false;
    }

    let Some(object) = function.child_by_field_name("object") else {
        return false;
    };
    let Some(property) = function.child_by_field_name("property") else {
        return false;
    };

    object.kind() == "identifier"
        && property.kind() == "property_identifier"
        && node_text(object, source) == Some("Developer")
        && node_text(property, source) == Some("shell")
}

fn shell_command_from_call(node: tree_sitter::Node<'_>, source: &str) -> Option<String> {
    let arguments = node.child_by_field_name("arguments")?;
    if arguments.named_child_count() != 1 {
        return None;
    }

    let object = arguments.named_child(0)?;
    if object.kind() != "object" || object.named_child_count() != 1 {
        return None;
    }

    let pair = object.named_child(0)?;
    if pair.kind() != "pair" {
        return None;
    }

    let key = pair.child_by_field_name("key")?;
    let is_command_key = match key.kind() {
        "property_identifier" => node_text(key, source) == Some("command"),
        "string" => node_text(key, source)
            .and_then(decode_quoted_string)
            .is_some_and(|key| key == "command"),
        _ => false,
    };
    if !is_command_key {
        return None;
    }

    let value = pair.child_by_field_name("value")?;
    if value.kind() != "string" {
        return None;
    }

    node_text(value, source).and_then(decode_quoted_string)
}

fn extract_shell_command_from_execute_code(code: &str) -> Option<String> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into())
        .ok()?;
    let tree = parser.parse(code, None)?;
    let root = tree.root_node();
    if root.has_error() {
        return None;
    }

    let mut command = None;
    let mut nodes = vec![root];
    while let Some(node) = nodes.pop() {
        if node.kind() == "call_expression" && is_developer_shell_call(node, code) {
            if command.is_some() {
                return None;
            }
            command = Some(shell_command_from_call(node, code)?);
        }

        let mut cursor = node.walk();
        nodes.extend(node.named_children(&mut cursor));
    }

    command
}

fn maybe_convert_execute_to_shell_tool_call(
    raw_tool_name: &str,
    arguments_value: &Value,
    tools: &[Tool],
) -> Option<CallToolRequestParams> {
    let alias = normalized_tool_alias(raw_tool_name);
    if alias != "execute" && alias != "execute_code" {
        return None;
    }

    let shell_tool_name = resolve_tool_name("shell", tools)?;
    let code = arguments_value.get("code")?.as_str()?;
    let command = extract_shell_command_from_execute_code(code)?;

    let shell_args = json!({ "command": command });
    Some(CallToolRequestParams::new(shell_tool_name).with_arguments(object(shell_args)))
}

fn escape_invalid_backslashes_in_json_strings(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 8);
    let mut in_string = false;
    let mut escaped = false;

    for ch in input.chars() {
        if in_string {
            if escaped {
                if !matches!(ch, '"' | '\\' | '/' | 'b' | 'f' | 'n' | 'r' | 't' | 'u') {
                    out.push('\\');
                }
                out.push(ch);
                escaped = false;
                continue;
            }

            match ch {
                '\\' => {
                    out.push('\\');
                    escaped = true;
                }
                '"' => {
                    out.push('"');
                    in_string = false;
                }
                _ => out.push(ch),
            }
            continue;
        }

        if ch == '"' {
            in_string = true;
        }
        out.push(ch);
    }

    if escaped {
        out.push('\\');
    }

    out
}

fn parse_json_value_tolerant(input: &str) -> Option<Value> {
    serde_json::from_str::<Value>(input).ok().or_else(|| {
        let escaped = escape_invalid_backslashes_in_json_strings(input);
        serde_json::from_str::<Value>(&escaped).ok()
    })
}

struct TokenizedToolCallParse {
    calls: Vec<CallToolRequestParams>,
    rejected_execute: bool,
}

#[allow(clippy::string_slice)] // All markers are ASCII; byte indexing is safe.
fn parse_tokenized_tool_calls_with_status(content: &str, tools: &[Tool]) -> TokenizedToolCallParse {
    let mut calls = Vec::new();
    let mut rejected_execute = false;
    let mut remainder = content;

    while let Some(begin_idx) = remainder.find(TOOL_CALL_BEGIN) {
        let after_begin = &remainder[begin_idx + TOOL_CALL_BEGIN.len()..];

        // Find the end of this tool call first
        let Some(call_end_offset) = after_begin.find(TOOL_CALL_END) else {
            let argument_marker = after_begin.find(TOOL_CALL_ARGUMENT_BEGIN);
            let brace_start = after_begin.find('{');
            let name_end = argument_marker.or(brace_start).unwrap_or(after_begin.len());
            let arguments_start = argument_marker
                .map(|argument_marker| argument_marker + TOOL_CALL_ARGUMENT_BEGIN.len())
                .or(brace_start);
            let arguments_contain_execute = arguments_start.is_some_and(|arguments_start| {
                malformed_arguments_contain_unresolved_execute_alias(
                    &after_begin[arguments_start..],
                    tools,
                )
            });
            if contains_unresolved_execute_alias(&after_begin[..name_end], tools)
                || arguments_contain_execute
            {
                rejected_execute = true;
            }
            break;
        };
        let call_body = &after_begin[..call_end_offset];

        // Try standard format: name <|tool_call_argument_begin|> {json}
        // Fall back to: name {json} (no argument marker)
        let (raw_tool_name, raw_args) =
            if let Some(arg_idx) = call_body.find(TOOL_CALL_ARGUMENT_BEGIN) {
                let name = call_body[..arg_idx].trim();
                let args = call_body[arg_idx + TOOL_CALL_ARGUMENT_BEGIN.len()..].trim();
                (name, args)
            } else if let Some(json_start) = call_body.find('{') {
                let name = call_body[..json_start].trim();
                let args = call_body[json_start..].trim();
                (name, args)
            } else {
                if contains_unresolved_execute_alias(call_body, tools) {
                    rejected_execute = true;
                }
                remainder = &after_begin[call_end_offset + TOOL_CALL_END.len()..];
                continue;
            };

        let resolved_tool_name = resolve_tool_name(raw_tool_name, tools);
        let is_execute_compatibility = contains_unresolved_execute_alias(raw_tool_name, tools);
        let raw_arguments_contain_structured_execute =
            raw_arguments_contain_structured_unresolved_execute_alias(raw_args, tools);

        if let Some(arguments_value) = parse_json_value_tolerant(raw_args) {
            let structured_execute_name = arguments_value
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| contains_unresolved_execute_alias(name, tools));
            let contains_structured_execute =
                contains_structured_unresolved_execute_alias(&arguments_value, tools);
            if is_execute_compatibility {
                if let Some(shell_call) =
                    maybe_convert_execute_to_shell_tool_call(raw_tool_name, &arguments_value, tools)
                {
                    calls.push(shell_call);
                } else {
                    rejected_execute = true;
                }
            } else if let Some(tool_name) = resolved_tool_name {
                if arguments_value.is_object() {
                    calls.push(
                        CallToolRequestParams::new(tool_name)
                            .with_arguments(object(arguments_value.clone())),
                    );
                }
            } else if let Some(structured_execute_name) = structured_execute_name {
                if let Some(shell_call) = arguments_value.get("arguments").and_then(|arguments| {
                    maybe_convert_execute_to_shell_tool_call(
                        structured_execute_name,
                        arguments,
                        tools,
                    )
                }) {
                    calls.push(shell_call);
                } else {
                    rejected_execute = true;
                }
            } else if contains_structured_execute || raw_arguments_contain_structured_execute {
                rejected_execute = true;
            }
        } else if is_execute_compatibility
            || raw_arguments_contain_structured_execute
            || malformed_arguments_contain_unresolved_execute_alias(raw_args, tools)
        {
            rejected_execute = true;
        }

        remainder = &after_begin[call_end_offset + TOOL_CALL_END.len()..];
    }

    TokenizedToolCallParse {
        calls,
        rejected_execute,
    }
}

#[cfg(test)]
fn parse_tokenized_tool_calls(content: &str, tools: &[Tool]) -> Vec<CallToolRequestParams> {
    parse_tokenized_tool_calls_with_status(content, tools).calls
}

#[allow(clippy::string_slice)] // Indices come from char_indices(); slicing is safe.
fn extract_first_json_object(input: &str) -> Option<(&str, usize)> {
    if !input.starts_with('{') {
        return None;
    }

    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for (idx, ch) in input.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
                continue;
            }
            match ch {
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    let end = idx + ch.len_utf8();
                    return Some((&input[..end], end));
                }
            }
            _ => {}
        }
    }

    None
}

#[allow(clippy::string_slice)] // Indices from find('{') on ASCII; byte slicing is safe.
fn parse_inline_json_tool_calls(content: &str, tools: &[Tool]) -> Vec<CallToolRequestParams> {
    let mut calls = Vec::new();
    let mut remainder = content;

    while let Some(start_idx) = remainder.find('{') {
        let maybe_json = &remainder[start_idx..];
        let Some((json_obj, consumed_len)) = extract_first_json_object(maybe_json) else {
            break;
        };

        if let Some(value) = parse_json_value_tolerant(json_obj) {
            let maybe_name = value.get("name").and_then(Value::as_str);
            let maybe_args = value.get("arguments").and_then(Value::as_object);
            if let (Some(raw_name), Some(arguments)) = (maybe_name, maybe_args) {
                if let Some(tool_name) = resolve_tool_name(raw_name, tools) {
                    calls.push(
                        CallToolRequestParams::new(tool_name).with_arguments(arguments.clone()),
                    );
                }
            }
        }

        remainder = &maybe_json[consumed_len..];
    }

    calls
}

#[allow(clippy::string_slice)] // Marker constants are ASCII; byte indexing is safe.
fn strip_tokenized_tool_markup(content: &str) -> String {
    let mut stripped = content.to_string();

    while let Some(section_start) = stripped.find(TOOL_CALLS_SECTION_BEGIN) {
        let after_start = section_start + TOOL_CALLS_SECTION_BEGIN.len();
        if let Some(section_end_rel) = stripped[after_start..].find(TOOL_CALLS_SECTION_END) {
            let section_end = after_start + section_end_rel + TOOL_CALLS_SECTION_END.len();
            stripped.replace_range(section_start..section_end, "");
        } else {
            stripped.replace_range(section_start..stripped.len(), "");
            break;
        }
    }

    for marker in [
        TOOL_CALL_BEGIN,
        TOOL_CALL_ARGUMENT_BEGIN,
        TOOL_CALL_ARGUMENT_END,
        TOOL_CALL_END,
        TOOL_CALLS_SECTION_BEGIN,
        TOOL_CALLS_SECTION_END,
    ] {
        stripped = stripped.replace(marker, " ");
    }

    stripped.trim().to_string()
}

fn append_tool_calls_to_message(
    mut message: Message,
    tool_calls: Vec<CallToolRequestParams>,
) -> Message {
    for tool_call in tool_calls {
        if tool_call.name != "noop" {
            let id = Uuid::new_v4().to_string();
            message = message.with_tool_request(id, Ok(tool_call));
        }
    }
    message
}

fn sanitize_message_after_tokenized_parse(mut message: Message) -> Message {
    for content in &mut message.content {
        if let MessageContent::Text(text) = content {
            text.text = strip_tokenized_tool_markup(&text.text);
        }
    }

    message.content.retain(|content| match content {
        MessageContent::Text(text) => !text.text.trim().is_empty(),
        _ => true,
    });

    message
}

fn sanitize_message_after_json_tool_parse(mut message: Message) -> Message {
    for content in &mut message.content {
        if let MessageContent::Text(text) = content {
            let lower = text.text.to_ascii_lowercase();
            let looks_like_tool_directive = lower.contains("using tool:")
                || (text.text.contains("\"name\"") && text.text.contains("\"arguments\""));

            if looks_like_tool_directive {
                text.text.clear();
            }
        }
    }

    message.content.retain(|content| match content {
        MessageContent::Text(text) => !text.text.trim().is_empty(),
        _ => true,
    });

    message
}

/// Returns `true` if the text contains any raw tool-use markers that should
/// never appear in final assistant output.
fn has_tool_markers(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    for marker in [
        TOOL_CALLS_SECTION_BEGIN,
        TOOL_CALLS_SECTION_END,
        TOOL_CALL_BEGIN,
        TOOL_CALL_ARGUMENT_BEGIN,
        TOOL_CALL_ARGUMENT_END,
        TOOL_CALL_END,
    ] {
        if text.contains(marker) {
            return true;
        }
    }
    lower.contains("using tool:") || (text.contains("\"name\"") && text.contains("\"arguments\""))
}

/// Catch-all sanitization applied to every message leaving the toolshim
/// pipeline, regardless of whether tool-call parsing succeeded.
pub fn sanitize_residual_markers(mut message: Message) -> Message {
    let mut changed = false;
    for content in &mut message.content {
        if let MessageContent::Text(text) = content {
            if has_tool_markers(&text.text) {
                // Strip tokenized markers first (handles section blocks)
                text.text = strip_tokenized_tool_markup(&text.text);
                // Then clear any remaining JSON-style tool directives
                let lower = text.text.to_ascii_lowercase();
                if lower.contains("using tool:")
                    || (text.text.contains("\"name\"") && text.text.contains("\"arguments\""))
                {
                    text.text.clear();
                }
                changed = true;
            }
        }
    }
    if changed {
        message.content.retain(|content| match content {
            MessageContent::Text(text) => !text.text.trim().is_empty(),
            _ => true,
        });
    }
    message
}

/// Environment variables that affect behavior:
/// - GOOSE_TOOLSHIM: When set to "true" or "1", enables using the tool shim in the standard OllamaProvider (default: false)
/// - GOOSE_TOOLSHIM_OLLAMA_MODEL: Ollama model to use as the tool interpreter (default: DEFAULT_INTERPRETER_MODEL)
/// A trait for models that can interpret text into structured tool call JSON format
#[async_trait::async_trait]
pub trait ToolInterpreter {
    /// Interpret potential tool calls from text and convert them to proper tool call JSON format
    async fn interpret_to_tool_calls(
        &self,
        content: &str,
        tools: &[Tool],
    ) -> Result<Vec<CallToolRequestParams>, ProviderError>;
}

/// Ollama-specific implementation of the ToolInterpreter trait
pub struct OllamaInterpreter {
    client: Client,
    base_url: String,
}

/// Local llama.cpp implementation of the ToolInterpreter trait.
pub struct LocalInterpreter {
    model: String,
}

impl LocalInterpreter {
    pub fn new() -> Result<Self, ProviderError> {
        Ok(Self {
            model: resolve_local_interpreter_model()?,
        })
    }

    async fn infer_structured_response(
        &self,
        format_instruction: &str,
    ) -> Result<String, ProviderError> {
        let model_config = crate::model_config::model_config_from_user_config("local", &self.model)
            .map_err(|e| ProviderError::RequestFailed(format!("Model config error: {e}")))?
            .with_toolshim(false)
            .with_toolshim_model(None);

        let provider = crate::providers::init::create("local", vec![])
            .await
            .map_err(|e| {
                ProviderError::RequestFailed(format!(
                    "Failed to create local interpreter provider: {e}"
                ))
            })?;

        let request_messages = vec![Message::user().with_text(format_instruction)];
        let mut stream = provider
            .stream(&model_config, "", &request_messages, &[])
            .await?;

        let mut content = String::new();
        while let Some(chunk) = stream.next().await {
            let (message, _) = chunk?;
            if let Some(message) = message {
                for part in message.content {
                    if let MessageContent::Text(text) = part {
                        content.push_str(&text.text);
                    }
                }
            }
        }

        Ok(content)
    }
}

impl OllamaInterpreter {
    pub fn new() -> Result<Self, ProviderError> {
        let client = Client::builder()
            .timeout(Duration::from_secs(DEFAULT_PROVIDER_TIMEOUT_SECS))
            .build()
            .expect("Failed to create HTTP client");

        let base_url = Self::get_ollama_base_url()?;

        Ok(Self { client, base_url })
    }

    /// Get the Ollama base URL from existing config or use default values
    fn get_ollama_base_url() -> Result<String, ProviderError> {
        let config = crate::config::Config::global();
        let host: String = config
            .get_param("OLLAMA_HOST")
            .unwrap_or_else(|_| OLLAMA_HOST.to_string());

        // Format the URL correctly with http:// prefix if needed
        let base = if host.starts_with("http://") || host.starts_with("https://") {
            &host
        } else {
            &format!("http://{}", host)
        };

        let mut base_url = url::Url::parse(base)
            .map_err(|e| ProviderError::RequestFailed(format!("Invalid base URL: {e}")))?;

        // Set the default port if missing
        // Don't add default port if:
        // 1. URL explicitly ends with standard ports (:80 or :443)
        // 2. URL uses HTTPS (which implicitly uses port 443)
        let explicit_default_port = host.ends_with(":80") || host.ends_with(":443");
        let is_https = base_url.scheme() == "https";

        if base_url.port().is_none() && !explicit_default_port && !is_https {
            base_url.set_port(Some(OLLAMA_DEFAULT_PORT)).map_err(|_| {
                ProviderError::RequestFailed("Failed to set default port".to_string())
            })?;
        }

        Ok(base_url.to_string())
    }

    fn tool_structured_output_format_schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "tool_calls": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": {
                                "type": "string",
                                "description": "The name of the tool to call"
                            },
                            "arguments": {
                                "type": "object",
                                "description": "The arguments to pass to the tool"
                            }
                        },
                        "required": ["name", "arguments"]
                    }
                }
            },
            "required": ["tool_calls"]
        })
    }

    async fn post_structured(
        &self,
        system_prompt: &str,
        format_instruction: &str,
        format_schema: Value,
        model: &str,
    ) -> Result<Value, ProviderError> {
        let base_url = self.base_url.trim_end_matches('/');
        let url = format!("{}/api/chat", base_url);

        let mut messages = Vec::new();
        let user_message = Message::user().with_text(format_instruction);
        messages.push(user_message);

        let model_config = model_config_from_user_config("ollama", model)?;

        let mut payload = create_request(
            &model_config,
            system_prompt,
            &messages,
            &[], // No tools
            &ImageFormat::OpenAi,
            false,
        )?;

        payload["stream"] = json!(false); // needed for the /api/chat endpoint to work
        payload["format"] = format_schema;

        tracing::info!(
            "Tool interpreter payload: {}",
            serde_json::to_string_pretty(&payload).unwrap_or_default()
        );

        let response = self.client.post(&url).json(&payload).send().await?;

        if !response.status().is_success() {
            let status = response.status();

            let error_text = match response.text().await {
                Ok(text) => text,
                Err(_) => "Could not read error response".to_string(),
            };

            return Err(ProviderError::RequestFailed(format!(
                "Ollama structured API returned error status {}: {}",
                status, error_text
            )));
        }

        let response_json: Value = response.json().await.map_err(|e| {
            ProviderError::RequestFailed(format!(
                "Failed to parse Ollama structured API response: {e}"
            ))
        })?;

        Ok(response_json)
    }

    fn process_interpreter_response(
        response: &Value,
    ) -> Result<Vec<CallToolRequestParams>, ProviderError> {
        let mut tool_calls = Vec::new();
        tracing::info!(
            "Tool interpreter response is {}",
            serde_json::to_string_pretty(&response).unwrap_or_default()
        );
        // Extract tool_calls array from the response
        if response.get("message").is_some() && response["message"].get("content").is_some() {
            let content = response["message"]["content"].as_str().unwrap_or_default();

            // Try to parse the content as JSON
            if let Ok(content_json) = serde_json::from_str::<Value>(content) {
                // Check for the format with tool_calls array inside an object
                if content_json.is_object() && content_json.get("tool_calls").is_some() {
                    // Process each tool call in the array
                    if let Some(tool_calls_array) = content_json["tool_calls"].as_array() {
                        for item in tool_calls_array {
                            if item.is_object()
                                && item.get("name").is_some()
                                && item.get("arguments").is_some()
                            {
                                let name = item["name"].as_str().unwrap_or_default().to_string();
                                let arguments = item["arguments"].clone();

                                // Add the tool call to our result vector
                                tool_calls.push(
                                    CallToolRequestParams::new(name)
                                        .with_arguments(object(arguments)),
                                );
                            }
                        }
                    }
                }
            }
        }

        Ok(tool_calls)
    }
}

#[async_trait::async_trait]
impl ToolInterpreter for OllamaInterpreter {
    async fn interpret_to_tool_calls(
        &self,
        last_assistant_msg: &str,
        tools: &[Tool],
    ) -> Result<Vec<CallToolRequestParams>, ProviderError> {
        if tools.is_empty() {
            return Ok(vec![]);
        }

        // Create the system prompt
        let system_prompt = "If there is detectable JSON-formatted tool requests, write them into valid JSON tool calls in the following format:
{{
  \"tool_calls\": [
    {{
      \"name\": \"tool_name\",
      \"arguments\": {{
        \"param1\": \"value1\",
        \"param2\": \"value2\"
      }}
    }}
  ]
}}

Otherwise, if no JSON tool requests are provided, use the no-op tool:
{{
  \"tool_calls\": [
    {{
    \"name\": \"noop\",
      \"arguments\": {{
      }}
    }}]
}}
";

        // Create enhanced content with instruction to output tool calls as JSON
        let format_instruction = format!("{}\nRequest: {}\n\n", system_prompt, last_assistant_msg);

        // Define the JSON schema for tool call format
        let format_schema = OllamaInterpreter::tool_structured_output_format_schema();

        // Determine which model to use for interpretation (from env var or default)
        let interpreter_model = std::env::var("GOOSE_TOOLSHIM_OLLAMA_MODEL")
            .unwrap_or_else(|_| DEFAULT_INTERPRETER_MODEL_OLLAMA.to_string());

        // Make a call to ollama with structured output
        let interpreter_response = self
            .post_structured("", &format_instruction, format_schema, &interpreter_model)
            .await?;

        // Process the interpreter response to get tool calls directly
        let tool_calls = OllamaInterpreter::process_interpreter_response(&interpreter_response)?;

        Ok(tool_calls)
    }
}

#[async_trait::async_trait]
impl ToolInterpreter for LocalInterpreter {
    async fn interpret_to_tool_calls(
        &self,
        last_assistant_msg: &str,
        tools: &[Tool],
    ) -> Result<Vec<CallToolRequestParams>, ProviderError> {
        if tools.is_empty() {
            return Ok(vec![]);
        }

        let system_prompt = "If there is detectable JSON-formatted tool requests, write them into valid JSON tool calls in the following format:
{{
    \"tool_calls\": [
        {{
            \"name\": \"tool_name\",
            \"arguments\": {{
                \"param1\": \"value1\",
                \"param2\": \"value2\"
            }}
        }}
    ]
}}

Otherwise, if no JSON tool requests are provided, use the no-op tool:
{{
    \"tool_calls\": [
        {{
        \"name\": \"noop\",
            \"arguments\": {{
            }}
        }}]
}}
";

        let format_instruction = format!("{}\nRequest: {}\n\n", system_prompt, last_assistant_msg);
        let content = self.infer_structured_response(&format_instruction).await?;
        let response = json!({ "message": { "content": content } });

        OllamaInterpreter::process_interpreter_response(&response)
    }
}

/// Creates a string containing formatted tool information
pub fn format_tool_info(tools: &[Tool]) -> String {
    let mut tool_info = String::new();
    for tool in tools {
        tool_info.push_str(&format!(
            "Tool Name: {}\nSchema: {}\nDescription: {:?}\n\n",
            tool.name,
            serde_json::to_string(&tool.input_schema).unwrap_or_default(),
            tool.description
        ));
    }
    tool_info
}

/// Convert messages containing ToolRequest/ToolResponse to text messages for toolshim mode
/// This is necessary because some providers (like Bedrock) validate that tool_use/tool_result
/// blocks can only exist when tools are defined, but in toolshim mode we pass empty tools
pub fn convert_tool_messages_to_text(messages: &[Message]) -> Conversation {
    let converted_messages: Vec<Message> = messages
        .iter()
        .map(|message| {
            let mut new_content = Vec::new();
            let mut has_tool_content = false;

            for content in &message.content {
                match content {
                    MessageContent::ToolRequest(req) => {
                        has_tool_content = true;
                        // Convert tool request to text format
                        let text = if let Ok(tool_call) = &req.tool_call {
                            format!(
                                "Using tool: {}\n{{\n  \"name\": \"{}\",\n  \"arguments\": {}\n}}",
                                tool_call.name,
                                tool_call.name,
                                serde_json::to_string_pretty(&tool_call.arguments)
                                    .unwrap_or_default()
                            )
                        } else {
                            "Tool request failed".to_string()
                        };
                        new_content.push(MessageContent::text(text));
                    }
                    MessageContent::ToolResponse(res) => {
                        has_tool_content = true;
                        // Convert tool response to text format
                        let text = match &res.tool_result {
                            Ok(result) => {
                                let text_contents: Vec<String> = result
                                    .content
                                    .iter()
                                    .filter_map(|c| match c {
                                        ContentBlock::Text(t) => Some(t.text.clone()),
                                        _ => None,
                                    })
                                    .collect();
                                format!("Tool result:\n{}", text_contents.join("\n"))
                            }
                            Err(e) => format!("Tool error: {}", e),
                        };
                        new_content.push(MessageContent::text(text));
                    }
                    _ => {
                        // Keep other content types as-is
                        new_content.push(content.clone());
                    }
                }
            }

            if has_tool_content {
                Message::new(message.role.clone(), message.created, new_content)
            } else {
                message.clone()
            }
        })
        .collect();

    Conversation::new_unvalidated(converted_messages)
}

/// Modifies the system prompt to include tool usage instructions when tool interpretation is enabled
pub fn modify_system_prompt_for_tool_json(system_prompt: &str, tools: &[Tool]) -> String {
    let tool_info = format_tool_info(tools);

    format!(
        "{}\n\n{}\n\nBreak down your task into smaller steps and do one step and tool call at a time. Do not try to use multiple tools at once. If you want to use a tool, tell the user what tool to use by specifying the tool in this JSON format\n{{\n  \"name\": \"tool_name\",\n  \"arguments\": {{\n    \"parameter1\": \"value1\",\n    \"parameter2\": \"value2\"\n }}\n}}. After you get the tool result back, consider the result and then proceed to do the next step and tool call if required.",
        system_prompt, tool_info
    )
}

/// Helper function to augment a message with tool calls if any are detected
pub async fn augment_message_with_tool_calls<T: ToolInterpreter>(
    interpreter: &T,
    message: Message,
    tools: &[Tool],
) -> Result<Message, ProviderError> {
    // If there are no tools or the message is empty, return the original message
    if tools.is_empty() {
        return Ok(message);
    }

    // Extract and combine all text content blocks from the message.
    let content = message
        .content
        .iter()
        .filter_map(|content| {
            if let MessageContent::Text(text) = content {
                Some(text.text.as_str())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    if content.trim().is_empty() {
        return Ok(message);
    }

    let has_existing_tool_request = message
        .content
        .iter()
        .any(|content| matches!(content, MessageContent::ToolRequest(_)));

    let direct_tool_calls = parse_tokenized_tool_calls_with_status(&content, tools);
    if direct_tool_calls.rejected_execute {
        return Ok(sanitize_message_after_tokenized_parse(message));
    }
    if !direct_tool_calls.calls.is_empty() {
        let cleaned = sanitize_message_after_tokenized_parse(message);
        return Ok(append_tool_calls_to_message(
            cleaned,
            direct_tool_calls.calls,
        ));
    }

    let inline_json_tool_calls = parse_inline_json_tool_calls(&content, tools);
    if !inline_json_tool_calls.is_empty() {
        let cleaned = sanitize_message_after_json_tool_parse(message);
        return Ok(append_tool_calls_to_message(
            cleaned,
            inline_json_tool_calls,
        ));
    }

    if has_existing_tool_request {
        return Ok(sanitize_residual_markers(message));
    }

    // Use the interpreter to convert the content to tool calls
    let tool_calls = interpreter.interpret_to_tool_calls(&content, tools).await?;

    // If no tool calls were detected, sanitize any residual markers
    if tool_calls.is_empty() {
        return Ok(sanitize_residual_markers(message));
    }

    Ok(sanitize_residual_markers(append_tool_calls_to_message(
        message, tool_calls,
    )))
}

pub async fn augment_message_with_selected_tool_interpreter(
    message: Message,
    tools: &[Tool],
) -> Result<Message, ProviderError> {
    match get_toolshim_backend()? {
        ToolshimBackend::Ollama => {
            let interpreter = OllamaInterpreter::new()?;
            augment_message_with_tool_calls(&interpreter, message, tools).await
        }
        ToolshimBackend::Local => {
            let interpreter = LocalInterpreter::new()?;
            augment_message_with_tool_calls(&interpreter, message, tools).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FailingInterpreter;

    #[async_trait::async_trait]
    impl ToolInterpreter for FailingInterpreter {
        async fn interpret_to_tool_calls(
            &self,
            _content: &str,
            _tools: &[Tool],
        ) -> Result<Vec<CallToolRequestParams>, ProviderError> {
            Err(ProviderError::RequestFailed(
                "interpreter should not be called".to_string(),
            ))
        }
    }

    #[test]
    fn formats_tool_schemas_as_compact_lossless_json() {
        let schema = object(
            json!({"type": "object", "properties": {"query": {"type": "string", "description": "café 工"}, "options": {"type": "object", "properties": {}}}}),
        );
        let tool = Tool::new("search".to_string(), "Search records".to_string(), schema);

        let output = format_tool_info(std::slice::from_ref(&tool));
        let description = format!("\nDescription: {:?}\n\n", tool.description);
        let schema = output
            .strip_prefix("Tool Name: search\nSchema: ")
            .and_then(|value| value.strip_suffix(&description))
            .unwrap();

        assert!(!schema.contains('\n'));
        let parsed: Value = serde_json::from_str(schema).unwrap();
        assert_eq!(parsed, serde_json::to_value(&tool.input_schema).unwrap());
    }

    #[test]
    fn parses_toolshim_backend_values() {
        assert_eq!(
            parse_toolshim_backend("ollama").unwrap(),
            ToolshimBackend::Ollama
        );
        assert_eq!(
            parse_toolshim_backend("local").unwrap(),
            ToolshimBackend::Local
        );
        assert_eq!(
            parse_toolshim_backend("llama.cpp").unwrap(),
            ToolshimBackend::Local
        );
        assert!(parse_toolshim_backend("something-else").is_err());
    }

    #[test]
    fn resolves_local_interpreter_model_prefers_env() {
        let model = resolve_local_interpreter_model_from_sources(
            Some("env-model".to_string()),
            Some("config-model".to_string()),
        )
        .unwrap();
        assert_eq!(model, "env-model");
    }

    #[test]
    fn resolves_local_interpreter_model_uses_config_fallback() {
        let model =
            resolve_local_interpreter_model_from_sources(None, Some("config-model".to_string()))
                .unwrap();
        assert_eq!(model, "config-model");
    }

    #[test]
    fn resolves_local_interpreter_model_requires_source() {
        assert!(resolve_local_interpreter_model_from_sources(None, None).is_err());
    }

    #[test]
    fn parses_tokenized_tool_calls() {
        let tools = vec![Tool::new(
            "shell".to_string(),
            "Shell command execution".to_string(),
            serde_json::Map::new(),
        )];

        let content = "<|tool_calls_section_begin|> <|tool_call_begin|> functions.shell:0 <|tool_call_argument_begin|> {\"command\":\"cat Cargo.toml\"} <|tool_call_end|> <|tool_calls_section_end|>";
        let calls = parse_tokenized_tool_calls(content, &tools);

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "shell");
        assert_eq!(
            calls[0]
                .arguments
                .as_ref()
                .and_then(|a| a.get("command"))
                .and_then(|v| v.as_str()),
            Some("cat Cargo.toml")
        );
    }

    #[test]
    fn parses_execute_marker_and_converts_to_shell_call() {
        let tools = vec![Tool::new(
            "shell".to_string(),
            "Shell command execution".to_string(),
            serde_json::Map::new(),
        )];

        let content = "<|tool_calls_section_begin|> <|tool_call_begin|> functions.execute:0 <|tool_call_argument_begin|> {\"code\":\"async function run() { const result = await Developer.shell({ command: \\\"cat Cargo.toml\\\" }); return result; }\"} <|tool_call_end|> <|tool_calls_section_end|>";

        let calls = parse_tokenized_tool_calls(content, &tools);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "shell");
        assert_eq!(
            calls[0]
                .arguments
                .as_ref()
                .and_then(|a| a.get("command"))
                .and_then(|v| v.as_str()),
            Some("cat Cargo.toml")
        );
    }

    #[test]
    fn execute_marker_ignores_command_data_before_shell_call() {
        let tools = vec![Tool::new(
            "shell".to_string(),
            "Shell command execution".to_string(),
            serde_json::Map::new(),
        )];

        let content = "<|tool_calls_section_begin|> <|tool_call_begin|> functions.execute:0 <|tool_call_argument_begin|> {\"code\":\"async function run() { const data = { command: \\\"cat /etc/shadow\\\" }; return await Developer.shell({ command: \\\"pwd\\\" }); }\"} <|tool_call_end|> <|tool_calls_section_end|>";

        let calls = parse_tokenized_tool_calls(content, &tools);

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "shell");
        assert_eq!(
            calls[0]
                .arguments
                .as_ref()
                .and_then(|a| a.get("command"))
                .and_then(|v| v.as_str()),
            Some("pwd")
        );
    }

    #[test]
    fn execute_code_only_extracts_real_developer_shell_call() {
        let code = r#"
            async function run() {
                const object = { command: "object decoy" };
                const text = 'Developer.shell({ command: "string decoy" })';
                const pattern = /Developer\.shell\(\{ command: "regex decoy" \}\)/;
                // Developer.shell({ command: "comment decoy" })
                return await Developer.shell({ "command": 'printf \'safe\'' });
            }
        "#;

        assert_eq!(
            extract_shell_command_from_execute_code(code).as_deref(),
            Some("printf 'safe'")
        );
    }

    #[test]
    fn execute_code_rejects_ambiguous_or_unsupported_shell_calls() {
        let cases = [
            "Developer.shell({ command: 'first' }); Developer.shell({ command: 'second' });",
            "Developer.shell({ command: getCommand() });",
            "Developer.shell({ command: 'pwd', timeout: 1000 });",
            "Developer['shell']({ command: 'pwd' });",
            "OtherDeveloper.shell({ command: 'pwd' });",
            "Developer.shellish({ command: 'pwd' });",
            "const text = 'Developer.shell({ command: \\\"pwd\\\" })';",
            "Developer.shell({ command: 'pwd'",
        ];

        for code in cases {
            assert_eq!(
                extract_shell_command_from_execute_code(code),
                None,
                "unexpectedly extracted a command from {code:?}"
            );
        }
    }

    #[test]
    fn parses_inline_json_tool_directive() {
        let tools = vec![Tool::new(
            "shell".to_string(),
            "Shell command execution".to_string(),
            serde_json::Map::new(),
        )];

        let content = "Using tool: shell\n{\n  \"name\": \"shell\",\n  \"arguments\": {\n    \"command\": \"type Cargo.toml\"\n  }\n}";
        let calls = parse_inline_json_tool_calls(content, &tools);

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "shell");
        assert_eq!(
            calls[0]
                .arguments
                .as_ref()
                .and_then(|a| a.get("command"))
                .and_then(|v| v.as_str()),
            Some("type Cargo.toml")
        );
    }

    #[test]
    fn parses_tokenized_tool_call_with_windows_path_arguments() {
        let tools = vec![Tool::new(
            "tree".to_string(),
            "Directory tree".to_string(),
            serde_json::Map::new(),
        )];

        let content = "<|tool_calls_section_begin|> <|tool_call_begin|> functions.tree:0 <|tool_call_argument_begin|> {\"path\": \"C:\\Users\\eugen\\programmazione\\goose-fork\", \"depth\": 1} <|tool_call_end|> <|tool_calls_section_end|>";
        let calls = parse_tokenized_tool_calls(content, &tools);

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "tree");
        assert_eq!(
            calls[0]
                .arguments
                .as_ref()
                .and_then(|a| a.get("path"))
                .and_then(|v| v.as_str()),
            Some("C:\\Users\\eugen\\programmazione\\goose-fork")
        );
    }

    #[tokio::test]
    async fn augment_uses_direct_tokenized_parser_before_interpreter() {
        let tools = vec![Tool::new(
            "shell".to_string(),
            "Shell command execution".to_string(),
            serde_json::Map::new(),
        )];

        let message = Message::assistant().with_text(
            "<|tool_calls_section_begin|> <|tool_call_begin|> functions.shell:0 <|tool_call_argument_begin|> {\"command\":\"cat Cargo.toml\"} <|tool_call_end|> <|tool_calls_section_end|>",
        );

        let augmented = augment_message_with_tool_calls(&FailingInterpreter, message, &tools)
            .await
            .unwrap();

        assert!(augmented
            .content
            .iter()
            .any(|c| matches!(c, MessageContent::ToolRequest(_))));
        assert!(!augmented.as_concat_text().contains("<|tool_call_begin|>"));
    }

    #[tokio::test]
    async fn augment_does_not_interpret_rejected_execute_marker() {
        let tools = vec![Tool::new(
            "shell".to_string(),
            "Shell command execution".to_string(),
            serde_json::Map::new(),
        )];
        let rejected = [
            "<|tool_calls_section_begin|> <|tool_call_begin|> functions.execute:0 <|tool_call_argument_begin|> {\"code\":\"Developer.shell({ command: 'first' }); Developer.shell({ command: 'second' });\"} <|tool_call_end|> <|tool_calls_section_end|>",
            "<|tool_calls_section_begin|> <|tool_call_begin|> functions.execute:0 <|tool_call_argument_begin|> {\"code\":\"Developer.shell({ command: getCommand() });\"} <|tool_call_end|> <|tool_calls_section_end|>",
            "<|tool_calls_section_begin|> <|tool_call_begin|> functions.execute:0 <|tool_call_argument_begin|> {\"code\": <|tool_call_end|> <|tool_calls_section_end|>",
            "<|tool_calls_section_begin|> <|tool_call_begin|> functions.execute:0 <|tool_call_end|> <|tool_calls_section_end|>",
            "<|tool_calls_section_begin|> <|tool_call_begin|> analysis:1 functions.execute:0 <|tool_call_end|> <|tool_calls_section_end|>",
            "<|tool_calls_section_begin|> <|tool_call_begin|> functions.execute:0 <|tool_call_argument_begin|> {\"code\":\"Developer.shell({ command: 'pwd' });\"}",
            "<|tool_calls_section_begin|> <|tool_call_begin|> label functions.execute:0 <|tool_call_argument_begin|> {\"code\":\"Developer.shell({ command: 'pwd' });\"}",
            "<|tool_calls_section_begin|> <|tool_call_begin|> analysis:1 functions.execute:0 <|tool_call_argument_begin|> {\"code\":\"Developer.shell({ command: 'pwd' });\"} <|tool_call_end|> <|tool_calls_section_end|>",
            "<|tool_calls_section_begin|> <|tool_call_begin|> analysis:1:functions.execute:0 <|tool_call_argument_begin|> {\"code\":\"Developer.shell({ command: 'pwd' });\"} <|tool_call_end|> <|tool_calls_section_end|>",
            "<|tool_calls_section_begin|> <|tool_call_begin|> label <|tool_call_argument_begin|> functions.execute:0 {\"code\":\"Developer.shell({ command: 'pwd' });\"} <|tool_call_end|> <|tool_calls_section_end|>",
            "<|tool_calls_section_begin|> <|tool_call_begin|> label <|tool_call_argument_begin|> {} functions.execute:0 {\"code\":\"Developer.shell({ command: 'pwd' });\"} <|tool_call_end|> <|tool_calls_section_end|>",
            "<|tool_calls_section_begin|> <|tool_call_begin|> shell:functions.execute:0 Developer.shell({ command: 'pwd' }) <|tool_call_end|> <|tool_calls_section_end|>",
        ];

        for content in rejected {
            assert!(
                parse_tokenized_tool_calls_with_status(content, &tools).rejected_execute,
                "expected execute block to be rejected: {content}"
            );
            let message = Message::assistant().with_text(content);
            let augmented = augment_message_with_tool_calls(&FailingInterpreter, message, &tools)
                .await
                .unwrap();

            assert!(augmented.content.is_empty());
        }
    }

    #[tokio::test]
    async fn augment_rejects_resolved_tool_prefix_with_execute_segment() {
        let tools = vec![Tool::new(
            "shell".to_string(),
            "Shell command execution".to_string(),
            serde_json::Map::new(),
        )];
        let message = Message::assistant().with_text(
            "<|tool_calls_section_begin|> <|tool_call_begin|> shell:functions.execute:0 <|tool_call_argument_begin|> {\"command\":\"id\"} <|tool_call_end|> <|tool_calls_section_end|>",
        );

        let augmented = augment_message_with_tool_calls(&FailingInterpreter, message, &tools)
            .await
            .unwrap();

        assert!(augmented.content.is_empty());
    }

    #[tokio::test]
    async fn augment_validates_execute_alias_with_call_punctuation() {
        let tools = vec![Tool::new(
            "shell".to_string(),
            "Shell command execution".to_string(),
            serde_json::Map::new(),
        )];
        let message = Message::assistant().with_text(
            "<|tool_calls_section_begin|> <|tool_call_begin|> functions.execute():0 <|tool_call_argument_begin|> {\"code\":\"Developer.shell({ command: 'id' })\"} <|tool_call_end|> <|tool_calls_section_end|>",
        );

        let augmented = augment_message_with_tool_calls(&FailingInterpreter, message, &tools)
            .await
            .unwrap();

        assert!(augmented
            .content
            .iter()
            .any(|content| matches!(content, MessageContent::ToolRequest(_))));
    }

    #[test]
    fn punctuated_benign_alias_is_not_rejected() {
        let tools = vec![Tool::new(
            "shell".to_string(),
            "Shell command execution".to_string(),
            serde_json::Map::new(),
        )];
        let content = "<|tool_calls_section_begin|> <|tool_call_begin|> functions.execute_helper():0 <|tool_call_argument_begin|> {\"value\":\"id\"} <|tool_call_end|> <|tool_calls_section_end|>";

        let parsed = parse_tokenized_tool_calls_with_status(content, &tools);

        assert!(parsed.calls.is_empty());
        assert!(!parsed.rejected_execute);
    }

    #[test]
    fn resolved_punctuated_tool_name_takes_precedence() {
        let tools = vec![Tool::new(
            "execute()".to_string(),
            "A distinct offered tool".to_string(),
            serde_json::Map::new(),
        )];
        let content = "<|tool_calls_section_begin|> <|tool_call_begin|> functions.execute():0 <|tool_call_argument_begin|> {\"value\":\"id\"} <|tool_call_end|> <|tool_calls_section_end|>";

        let parsed = parse_tokenized_tool_calls_with_status(content, &tools);

        assert!(!parsed.rejected_execute);
        assert_eq!(parsed.calls.len(), 1);
        assert_eq!(parsed.calls[0].name, "execute()");
    }

    #[tokio::test]
    async fn augment_rejects_execute_alias_after_malformed_opening_brace() {
        let tools = vec![Tool::new(
            "shell".to_string(),
            "Shell command execution".to_string(),
            serde_json::Map::new(),
        )];
        let message = Message::assistant().with_text(
            "<|tool_calls_section_begin|> <|tool_call_begin|> label <|tool_call_argument_begin|> {] functions.execute:0 {\"name\":\"shell\",\"arguments\":{\"command\":\"id\"}}} <|tool_call_end|> <|tool_calls_section_end|>",
        );

        let augmented = augment_message_with_tool_calls(&FailingInterpreter, message, &tools)
            .await
            .unwrap();

        assert!(augmented.content.is_empty());
    }

    #[tokio::test]
    async fn augment_rejects_execute_alias_in_unterminated_arguments() {
        let tools = vec![Tool::new(
            "shell".to_string(),
            "Shell command execution".to_string(),
            serde_json::Map::new(),
        )];
        let message = Message::assistant().with_text(
            "<|tool_calls_section_begin|> <|tool_call_begin|> label <|tool_call_argument_begin|> functions.execute:0 {\"code\":\"Developer.shell({ command: 'id' })\"}",
        );

        let augmented = augment_message_with_tool_calls(&FailingInterpreter, message, &tools)
            .await
            .unwrap();

        assert!(augmented.content.is_empty());
    }

    #[tokio::test]
    async fn augment_validates_structured_execute_name_without_interpreter() {
        let tools = vec![Tool::new(
            "shell".to_string(),
            "Shell command execution".to_string(),
            serde_json::Map::new(),
        )];
        let message = Message::assistant().with_text(
            "<|tool_calls_section_begin|> <|tool_call_begin|> label <|tool_call_argument_begin|> {\"name\":\"functions.execute:0\",\"arguments\":{\"code\":\"Developer.shell({ command: 'id' })\"}} <|tool_call_end|> <|tool_calls_section_end|>",
        );

        let augmented = augment_message_with_tool_calls(&FailingInterpreter, message, &tools)
            .await
            .unwrap();

        assert!(augmented
            .content
            .iter()
            .any(|content| matches!(content, MessageContent::ToolRequest(_))));
    }

    #[tokio::test]
    async fn augment_preserves_resolved_tool_with_execute_shaped_arguments() {
        let tools = vec![
            Tool::new(
                "workflow".to_string(),
                "Run a workflow".to_string(),
                serde_json::Map::new(),
            ),
            Tool::new(
                "shell".to_string(),
                "Shell command execution".to_string(),
                serde_json::Map::new(),
            ),
        ];
        let message = Message::assistant().with_text(
            "<|tool_calls_section_begin|> <|tool_call_begin|> functions.workflow:0 <|tool_call_argument_begin|> {\"name\":\"functions.execute:0\",\"arguments\":{\"code\":\"Developer.shell({ command: 'id' })\"}} <|tool_call_end|> <|tool_calls_section_end|>",
        );

        let augmented = augment_message_with_tool_calls(&FailingInterpreter, message, &tools)
            .await
            .unwrap();
        let tool_call = augmented
            .content
            .iter()
            .find_map(|content| match content {
                MessageContent::ToolRequest(request) => request.tool_call.as_ref().ok(),
                _ => None,
            })
            .unwrap();

        assert_eq!(tool_call.name, "workflow");
        assert_eq!(
            tool_call
                .arguments
                .as_ref()
                .and_then(|arguments| arguments.get("name"))
                .and_then(Value::as_str),
            Some("functions.execute:0")
        );
    }

    #[tokio::test]
    async fn augment_rejects_execute_envelope_in_argument_array() {
        let tools = vec![Tool::new(
            "shell".to_string(),
            "Shell command execution".to_string(),
            serde_json::Map::new(),
        )];
        let message = Message::assistant().with_text(
            "<|tool_calls_section_begin|> <|tool_call_begin|> label <|tool_call_argument_begin|> [{\"name\":\"functions.execute:0\",\"arguments\":{\"code\":\"Developer.shell({ command: 'id' })\"}}] <|tool_call_end|> <|tool_calls_section_end|>",
        );

        let augmented = augment_message_with_tool_calls(&FailingInterpreter, message, &tools)
            .await
            .unwrap();

        assert!(augmented.content.is_empty());
    }

    #[tokio::test]
    async fn augment_rejects_execute_envelope_in_nested_arguments() {
        let tools = vec![Tool::new(
            "shell".to_string(),
            "Shell command execution".to_string(),
            serde_json::Map::new(),
        )];
        let message = Message::assistant().with_text(
            "<|tool_calls_section_begin|> <|tool_call_begin|> label <|tool_call_argument_begin|> {\"payload\":{\"name\":\"functions.execute:0\",\"arguments\":{\"code\":\"Developer.shell({ command: 'id' })\"}}} <|tool_call_end|> <|tool_calls_section_end|>",
        );

        let augmented = augment_message_with_tool_calls(&FailingInterpreter, message, &tools)
            .await
            .unwrap();

        assert!(augmented.content.is_empty());
    }

    #[tokio::test]
    async fn augment_rejects_execute_envelope_before_malformed_suffix() {
        let tools = vec![Tool::new(
            "shell".to_string(),
            "Shell command execution".to_string(),
            serde_json::Map::new(),
        )];
        let message = Message::assistant().with_text(
            "<|tool_calls_section_begin|> <|tool_call_begin|> label <|tool_call_argument_begin|> {\"name\":\"functions.execute:0\",\"arguments\":{\"code\":\"Developer.shell({ command: 'id' })\"}} x <|tool_call_end|> <|tool_calls_section_end|>",
        );

        let augmented = augment_message_with_tool_calls(&FailingInterpreter, message, &tools)
            .await
            .unwrap();

        assert!(augmented.content.is_empty());
    }

    #[tokio::test]
    async fn augment_rejects_execute_name_overwritten_by_duplicate_key() {
        let tools = vec![Tool::new(
            "shell".to_string(),
            "Shell command execution".to_string(),
            serde_json::Map::new(),
        )];
        let message = Message::assistant().with_text(
            "<|tool_calls_section_begin|> <|tool_call_begin|> label <|tool_call_argument_begin|> {\"name\":\"functions.execute:0\",\"name\":\"transform\",\"arguments\":{\"code\":\"Developer.shell({ command: 'id' })\"}} <|tool_call_end|> <|tool_calls_section_end|>",
        );

        let augmented = augment_message_with_tool_calls(&FailingInterpreter, message, &tools)
            .await
            .unwrap();

        assert!(augmented.content.is_empty());
    }

    #[test]
    fn unresolved_header_does_not_reject_benign_duplicate_names() {
        let tools = vec![Tool::new(
            "shell".to_string(),
            "Shell command execution".to_string(),
            serde_json::Map::new(),
        )];
        let content = "<|tool_calls_section_begin|> <|tool_call_begin|> label <|tool_call_argument_begin|> {\"name\":\"first\",\"name\":\"transform\",\"arguments\":{\"value\":\"id\"}} <|tool_call_end|> <|tool_calls_section_end|>";

        let parsed = parse_tokenized_tool_calls_with_status(content, &tools);

        assert!(parsed.calls.is_empty());
        assert!(!parsed.rejected_execute);
    }

    #[test]
    fn resolved_tool_accepts_duplicate_execute_shaped_argument_names() {
        let tools = vec![
            Tool::new(
                "workflow".to_string(),
                "Run a workflow".to_string(),
                serde_json::Map::new(),
            ),
            Tool::new(
                "shell".to_string(),
                "Shell command execution".to_string(),
                serde_json::Map::new(),
            ),
        ];
        let content = "<|tool_calls_section_begin|> <|tool_call_begin|> functions.workflow:0 <|tool_call_argument_begin|> {\"name\":\"functions.execute:0\",\"name\":\"transform\",\"arguments\":{\"code\":\"Developer.shell({ command: 'id' })\"}} <|tool_call_end|> <|tool_calls_section_end|>";

        let parsed = parse_tokenized_tool_calls_with_status(content, &tools);

        assert!(!parsed.rejected_execute);
        assert_eq!(parsed.calls.len(), 1);
        assert_eq!(parsed.calls[0].name, "workflow");
    }

    #[test]
    fn resolved_tool_accepts_nested_execute_shaped_arguments() {
        let tools = vec![
            Tool::new(
                "workflow".to_string(),
                "Run a workflow".to_string(),
                serde_json::Map::new(),
            ),
            Tool::new(
                "shell".to_string(),
                "Shell command execution".to_string(),
                serde_json::Map::new(),
            ),
        ];
        let content = "<|tool_calls_section_begin|> <|tool_call_begin|> functions.workflow:0 <|tool_call_argument_begin|> {\"payload\":[{\"name\":\"functions.execute:0\",\"arguments\":{\"code\":\"Developer.shell({ command: 'id' })\"}}]} <|tool_call_end|> <|tool_calls_section_end|>";

        let parsed = parse_tokenized_tool_calls_with_status(content, &tools);

        assert!(!parsed.rejected_execute);
        assert_eq!(parsed.calls.len(), 1);
        assert_eq!(parsed.calls[0].name, "workflow");
    }

    #[test]
    fn unresolved_header_does_not_reject_benign_nested_json() {
        let tools = vec![Tool::new(
            "shell".to_string(),
            "Shell command execution".to_string(),
            serde_json::Map::new(),
        )];
        let content = "<|tool_calls_section_begin|> <|tool_call_begin|> label <|tool_call_argument_begin|> {\"payload\":[{\"name\":\"transform\",\"arguments\":{\"value\":\"id\"}}]} <|tool_call_end|> <|tool_calls_section_end|>";

        let parsed = parse_tokenized_tool_calls_with_status(content, &tools);

        assert!(parsed.calls.is_empty());
        assert!(!parsed.rejected_execute);
    }

    #[test]
    fn unresolved_header_does_not_reject_benign_json_prefix() {
        let tools = vec![Tool::new(
            "shell".to_string(),
            "Shell command execution".to_string(),
            serde_json::Map::new(),
        )];
        let content = "<|tool_calls_section_begin|> <|tool_call_begin|> label <|tool_call_argument_begin|> {\"payload\":{\"name\":\"transform\"}} x <|tool_call_end|> <|tool_calls_section_end|>";

        let parsed = parse_tokenized_tool_calls_with_status(content, &tools);

        assert!(parsed.calls.is_empty());
        assert!(!parsed.rejected_execute);
    }

    #[tokio::test]
    async fn augment_rejects_execute_alias_in_unterminated_brace_remainder() {
        let tools = vec![Tool::new(
            "shell".to_string(),
            "Shell command execution".to_string(),
            serde_json::Map::new(),
        )];
        let message = Message::assistant().with_text(
            "<|tool_calls_section_begin|> <|tool_call_begin|> label {] functions.execute:0 {\"code\":\"Developer.shell({ command: 'id' })\"}",
        );

        let augmented = augment_message_with_tool_calls(&FailingInterpreter, message, &tools)
            .await
            .unwrap();

        assert!(augmented.content.is_empty());
    }

    #[test]
    fn unterminated_non_execute_arguments_are_not_rejected() {
        let tools = vec![Tool::new(
            "shell".to_string(),
            "Shell command execution".to_string(),
            serde_json::Map::new(),
        )];
        let contents = [
            "<|tool_calls_section_begin|> <|tool_call_begin|> functions.shell:0 <|tool_call_argument_begin|> {\"command\":\"id\"}",
            "<|tool_calls_section_begin|> <|tool_call_begin|> functions.shell:0 {\"command\":\"id\"}",
        ];

        for content in contents {
            let parsed = parse_tokenized_tool_calls_with_status(content, &tools);

            assert!(parsed.calls.is_empty());
            assert!(!parsed.rejected_execute);
        }
    }

    #[tokio::test]
    async fn augment_parses_inline_json_even_with_existing_tool_request() {
        let tools = vec![
            Tool::new(
                "analyze".to_string(),
                "Analyze files".to_string(),
                serde_json::Map::new(),
            ),
            Tool::new(
                "shell".to_string(),
                "Shell command execution".to_string(),
                serde_json::Map::new(),
            ),
        ];

        let message = Message::assistant()
            .with_tool_request("existing", Ok(CallToolRequestParams::new("analyze")))
            .with_text(
                "Using tool: shell\n{\n  \"name\": \"shell\",\n  \"arguments\": {\n    \"command\": \"type Cargo.toml\"\n  }\n}",
            );

        let augmented = augment_message_with_tool_calls(&FailingInterpreter, message, &tools)
            .await
            .unwrap();

        let tool_request_count = augmented
            .content
            .iter()
            .filter(|c| matches!(c, MessageContent::ToolRequest(_)))
            .count();
        assert_eq!(tool_request_count, 2);
    }

    #[tokio::test]
    async fn augment_parses_tokenized_tool_call_from_later_text_chunk() {
        let tools = vec![Tool::new(
            "shell".to_string(),
            "Shell command execution".to_string(),
            serde_json::Map::new(),
        )];

        let message = Message::assistant()
            .with_text("I will inspect the file now.")
            .with_text(
                "<|tool_calls_section_begin|> <|tool_call_begin|> functions.shell:0 <|tool_call_argument_begin|> {\"command\":\"type Cargo.toml\"} <|tool_call_end|> <|tool_calls_section_end|>",
            );

        let augmented = augment_message_with_tool_calls(&FailingInterpreter, message, &tools)
            .await
            .unwrap();

        assert!(augmented
            .content
            .iter()
            .any(|c| matches!(c, MessageContent::ToolRequest(_))));
    }

    // ── Regression tests: malformed marker leakage ──────────────────────

    /// Malformed tokenized markers (incomplete/garbled) must be stripped
    /// from the final text even when parsing yields zero tool calls.
    #[tokio::test]
    async fn malformed_tokenized_markers_stripped_from_text_output() {
        let tools = vec![Tool::new(
            "shell".to_string(),
            "Shell command execution".to_string(),
            serde_json::Map::new(),
        )];

        // Marker sequence is incomplete — no TOOL_CALL_ARGUMENT_BEGIN,
        // so parse_tokenized_tool_calls returns empty.
        let message = Message::assistant().with_text(
            "Here is the result.\n<|tool_calls_section_begin|> <|tool_call_begin|> functions.shell:0 GARBAGE <|tool_call_end|> <|tool_calls_section_end|>",
        );

        // Use an interpreter that returns empty (simulates no-match fallback)
        struct EmptyInterpreter;
        #[async_trait::async_trait]
        impl ToolInterpreter for EmptyInterpreter {
            async fn interpret_to_tool_calls(
                &self,
                _content: &str,
                _tools: &[Tool],
            ) -> Result<Vec<CallToolRequestParams>, ProviderError> {
                Ok(vec![])
            }
        }

        let result = augment_message_with_tool_calls(&EmptyInterpreter, message, &tools)
            .await
            .unwrap();

        let text = result.as_concat_text();
        assert!(
            !has_tool_markers(&text),
            "Residual tokenized markers leaked into output: {text}"
        );
    }

    /// Malformed JSON-style tool directives ("Using tool: …" without valid
    /// JSON) must be stripped from the final text.
    #[tokio::test]
    async fn malformed_json_directive_stripped_from_text_output() {
        let tools = vec![Tool::new(
            "shell".to_string(),
            "Shell command execution".to_string(),
            serde_json::Map::new(),
        )];

        // "Using tool:" present but no valid JSON follows
        let message = Message::assistant().with_text(
            "I will run the command.\nUsing tool: shell\n{invalid json that won't parse}",
        );

        struct EmptyInterpreter;
        #[async_trait::async_trait]
        impl ToolInterpreter for EmptyInterpreter {
            async fn interpret_to_tool_calls(
                &self,
                _content: &str,
                _tools: &[Tool],
            ) -> Result<Vec<CallToolRequestParams>, ProviderError> {
                Ok(vec![])
            }
        }

        let result = augment_message_with_tool_calls(&EmptyInterpreter, message, &tools)
            .await
            .unwrap();

        let text = result.as_concat_text();
        assert!(
            !has_tool_markers(&text),
            "Residual JSON tool directive leaked into output: {text}"
        );
    }

    #[test]
    fn has_tool_markers_detects_tokenized_markers() {
        assert!(has_tool_markers("hello <|tool_calls_section_begin|> world"));
        assert!(has_tool_markers("text <|tool_call_begin|> more"));
        assert!(!has_tool_markers("clean assistant text with no markers"));
    }

    #[test]
    fn has_tool_markers_detects_json_directive() {
        assert!(has_tool_markers("Using tool: shell\n{...}"));
        assert!(has_tool_markers("blah \"name\" blah \"arguments\" blah"));
        assert!(!has_tool_markers("just normal text mentioning a name"));
    }

    #[test]
    fn parses_tokenized_tool_call_without_argument_marker() {
        let tools = vec![Tool::new(
            "Nadirclawusage__usageSummary".to_string(),
            "Usage summary".to_string(),
            serde_json::Map::new(),
        )];

        // Model emits tool call without <|tool_call_argument_begin|>
        let content = "<|tool_calls_section_begin|> <|tool_call_begin|> functions.Nadirclawusage.usageSummary:1  {\"period\": \"24h\"} <|tool_call_end|> <|tool_calls_section_end|>";
        let calls = parse_tokenized_tool_calls(content, &tools);

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "Nadirclawusage__usageSummary");
        assert_eq!(
            calls[0]
                .arguments
                .as_ref()
                .and_then(|a| a.get("period"))
                .and_then(|v| v.as_str()),
            Some("24h")
        );
    }
}
