use crate::images::ImageFormat;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::ops::Range;

use crate::api_client::ApiClient;

pub type OpenRouterSessionIdProvider = Box<dyn Fn() -> Option<String> + Send + Sync>;
use crate::base::{ConfigKey, MessageStream, Provider, ProviderMetadata};
use crate::cache_semantics::{apply_chat_payload_breakpoints, CacheSemantics};
use crate::conversation::message::Message;
use crate::errors::ProviderError;
use crate::formats::openai::create_request;
use crate::model::ModelConfig;
use crate::openai_compatible::{handle_status, stream_openai_compat};
use crate::openrouter_format;
use crate::request_log::{start_log, LoggerHandleExt};
use crate::retry::ProviderRetry;
use rmcp::model::Tool;

pub const OPENROUTER_PROVIDER_NAME: &str = "openrouter";
const OPENROUTER_PARAMETERS_CONFIG_KEY: &str = "OPENROUTER_PARAMETERS";
pub const OPENROUTER_DEFAULT_MODEL: &str = "anthropic/claude-sonnet-4";

// OpenRouter can run many models, we suggest the default
pub const OPENROUTER_KNOWN_MODELS: &[&str] = &[
    "x-ai/grok-code-fast-1",
    "anthropic/claude-sonnet-4.5",
    "anthropic/claude-sonnet-4",
    "anthropic/claude-opus-4.1",
    "anthropic/claude-opus-4",
    "google/gemini-2.5-pro",
    "google/gemini-2.5-flash",
    "deepseek/deepseek-r1-0528",
    "qwen/qwen3-coder",
    "moonshotai/kimi-k2",
];
pub const OPENROUTER_DOC_URL: &str = "https://openrouter.ai/models";

const GEMINI_SCHEMA_REF_KEY: &str = "$ref";
const GEMINI_SAFE_SCHEMA_REF_KEY_BASE: &str = "dollar_ref";

#[derive(serde::Serialize)]
pub struct OpenRouterProvider {
    #[serde(skip)]
    api_client: ApiClient,
    supports_streaming: bool,
    #[serde(skip)]
    name: String,
    #[serde(skip)]
    configured_parameters: Option<HashMap<String, Value>>,
    #[serde(skip)]
    session_id_provider: Option<OpenRouterSessionIdProvider>,
}

impl OpenRouterProvider {
    pub fn new(
        api_client: ApiClient,
        configured_parameters: Option<HashMap<String, Value>>,
        session_id_provider: Option<OpenRouterSessionIdProvider>,
    ) -> Self {
        Self {
            api_client,
            supports_streaming: true,
            name: OPENROUTER_PROVIDER_NAME.to_string(),
            configured_parameters,
            session_id_provider,
        }
    }

    async fn post_chat_completions(
        &self,
        model_config: &ModelConfig,
        payload: &Value,
    ) -> Result<reqwest::Response, ProviderError> {
        self.with_retry(|| async {
            let resp = self
                .api_client
                .request("api/v1/chat/completions")
                .model_headers(model_config)?
                .streaming(true)
                .response_post(payload)
                .await?;
            handle_status(resp).await
        })
        .await
    }
}

fn is_mandatory_reasoning_error(error: &ProviderError) -> bool {
    matches!(error, ProviderError::RequestFailed(message) if message.contains("Reasoning is mandatory"))
}

fn is_gemini_model(model_name: &str) -> bool {
    model_name.starts_with("google/gemini")
}

/// Spans of the literal `$ref` token inside opaque tool text.
///
/// Tool results are not required to parse as JSON. Google rejects the token in
/// single-quoted Python `repr` output, YAML, and unquoted text just as it does
/// in strict JSON, so matching only well-formed JSON key positions would miss
/// the reproduction in #11260. A trailing identifier character means the token
/// is part of a longer name such as `$refs` or `$refresh_token`, which must be
/// left alone.
fn scan_schema_ref_tokens(content: &str) -> Vec<Range<usize>> {
    let mut spans = Vec::new();
    let mut cursor = 0;

    while cursor < content.len() {
        if !content.is_char_boundary(cursor) {
            cursor += 1;
            continue;
        }

        match match_schema_ref_at(content, cursor) {
            Some(end)
                if !starts_with_escaped_backslash(content, cursor)
                    && !continues_identifier(content, end) =>
            {
                spans.push(cursor..end);
                cursor = end;
            }
            _ => {
                cursor += content
                    .get(cursor..)
                    .and_then(|rest| rest.chars().next())
                    .map_or(1, char::len_utf8);
            }
        }
    }

    spans
}

/// Decodes one character: either a literal one or a `\uXXXX` escape. JSON lets
/// any character of a key be escaped, so `$\u0072ef` and `\u0024\u0072\u0065\u0066`
/// both decode to `$ref` and would otherwise reach Gemini unrewritten.
fn decode_unit(content: &str, index: usize) -> Option<(char, usize)> {
    let rest = content.get(index..)?;

    if let Some(after_prefix) = rest.strip_prefix("\\u") {
        let character = after_prefix
            .get(..4)
            .and_then(|hex| u32::from_str_radix(hex, 16).ok())
            .and_then(char::from_u32)?;
        return Some((character, index + "\\u".len() + 4));
    }

    let character = rest.chars().next()?;
    Some((character, index + character.len_utf8()))
}

/// Returns the end offset when the text at `start` decodes to `$ref`.
fn match_schema_ref_at(content: &str, start: usize) -> Option<usize> {
    let mut cursor = start;

    for expected in GEMINI_SCHEMA_REF_KEY.chars() {
        let (character, next) = decode_unit(content, cursor)?;
        if character != expected {
            return None;
        }
        cursor = next;
    }

    Some(cursor)
}

/// An odd number of backslashes before `\u0024ref` means the leading backslash
/// is itself escaped, so the text is the literal characters `\u0024ref` rather
/// than an encoded `$ref`. Rewriting it would emit invalid JSON.
fn starts_with_escaped_backslash(content: &str, start: usize) -> bool {
    if !content
        .get(start..)
        .is_some_and(|token| token.starts_with('\\'))
    {
        return false;
    }

    let preceding_backslashes = content
        .get(..start)
        .map(|prefix| prefix.chars().rev().take_while(|c| *c == '\\').count())
        .unwrap_or(0);

    preceding_backslashes % 2 == 1
}

/// A trailing identifier character means the token is part of a longer name
/// such as `$refs` or `$refresh_token`, which must be left alone.
fn continues_identifier(content: &str, end: usize) -> bool {
    decode_unit(content, end)
        .is_some_and(|(character, _)| character.is_ascii_alphanumeric() || character == '_')
}

/// Decodes `\uXXXX` escapes so a candidate key spelled as an escape sequence
/// still counts as occupied.
fn decoded_view(content: &str) -> String {
    let mut decoded = String::with_capacity(content.len());
    let mut rest = content;

    while let Some(offset) = rest.find("\\u") {
        let (before, from_escape) = rest.split_at(offset);
        decoded.push_str(before);

        let after_prefix = from_escape.get("\\u".len()..).unwrap_or_default();
        match after_prefix
            .get(..4)
            .and_then(|hex| u32::from_str_radix(hex, 16).ok())
            .and_then(char::from_u32)
        {
            Some(character) => {
                decoded.push(character);
                rest = after_prefix.get(4..).unwrap_or_default();
            }
            None => {
                decoded.push_str("\\u");
                rest = after_prefix;
            }
        }
    }
    decoded.push_str(rest);

    decoded
}

fn replace_spans(content: &str, spans: &[Range<usize>], replacement: &str) -> String {
    let mut rewritten = String::with_capacity(content.len());
    let mut copied_through = 0;

    for span in spans {
        if let Some(preceding) = content.get(copied_through..span.start) {
            rewritten.push_str(preceding);
        }
        rewritten.push_str(replacement);
        copied_through = span.end;
    }
    if let Some(trailing) = content.get(copied_through..) {
        rewritten.push_str(trailing);
    }

    rewritten
}

/// The replacement must not already occur anywhere in the tool text, otherwise
/// the rewrite would be ambiguous to the model reading the compatibility note.
///
/// Tool output is externally controlled, so occupied candidates are collected in
/// a single pass rather than rescanning every result for each suffix.
fn collision_free_gemini_schema_ref_key(contents: &[&str]) -> String {
    let mut occupied = HashSet::new();
    for content in contents {
        collect_safe_key_candidates(content, &mut occupied);
        collect_safe_key_candidates(&decoded_view(content), &mut occupied);
    }

    (1..)
        .map(|suffix| {
            if suffix == 1 {
                GEMINI_SAFE_SCHEMA_REF_KEY_BASE.to_string()
            } else {
                format!("{GEMINI_SAFE_SCHEMA_REF_KEY_BASE}_{suffix}")
            }
        })
        .find(|candidate| !occupied.contains(candidate))
        .expect("an unbounded suffix sequence always yields an unused candidate")
}

/// Records every `dollar_ref`/`dollar_ref_N` occurrence in one scan.
fn collect_safe_key_candidates(content: &str, occupied: &mut HashSet<String>) {
    let mut cursor = 0;

    while let Some(offset) = content
        .get(cursor..)
        .and_then(|tail| tail.find(GEMINI_SAFE_SCHEMA_REF_KEY_BASE))
    {
        let start = cursor + offset;
        let end = start + GEMINI_SAFE_SCHEMA_REF_KEY_BASE.len();
        let suffix_len = content
            .get(end..)
            .map(|rest| {
                let digits = rest
                    .strip_prefix('_')
                    .map(|after| after.chars().take_while(char::is_ascii_digit).count())
                    .unwrap_or(0);
                if digits == 0 {
                    0
                } else {
                    1 + digits
                }
            })
            .unwrap_or(0);

        if let Some(token) = content.get(start..end + suffix_len) {
            occupied.insert(token.to_string());
        }
        cursor = end;
    }
}

fn gemini_schema_ref_note(safe_key: &str) -> String {
    format!(
        "[OpenRouter/Gemini compatibility: interpret `{safe_key}` as the JSON Schema key formed by `$` followed by `ref`.]\n"
    )
}

fn apply_gemini_compatibility(model_name: &str, payload: &mut Value, messages: &[Message]) {
    if is_gemini_model(model_name) {
        escape_gemini_schema_ref_keys_in_tool_responses(payload);
        openrouter_format::add_reasoning_details_to_request(payload, messages);
    }
}

/// OpenRouter translates OpenAI `role: tool` messages into Gemini
/// `function_response` parts. Gemini rejects a response containing a literal
/// JSON Schema `$ref` key, treating its value as a function-response part name
/// instead of arbitrary tool text, and Goose replays persisted history, so one
/// such tool result breaks every later turn in the session.
///
/// Rewrite the token and prepend a note so the model can reconstruct the
/// original text. All bytes outside matching token spans remain unchanged.
fn escape_gemini_schema_ref_keys_in_tool_responses(payload: &mut Value) -> usize {
    let Some(messages) = payload.get_mut("messages").and_then(Value::as_array_mut) else {
        return 0;
    };

    let mut tool_contents = Vec::new();
    for (message_index, message) in messages.iter().enumerate() {
        if message.get("role").and_then(Value::as_str) != Some("tool") {
            continue;
        }
        let Some(content_text) = message.get("content").and_then(Value::as_str) else {
            continue;
        };
        tool_contents.push((message_index, content_text.to_string()));
    }

    let scanned: Vec<&str> = tool_contents
        .iter()
        .map(|(_, content)| content.as_str())
        .collect();
    let safe_key = collision_free_gemini_schema_ref_key(&scanned);
    let note = gemini_schema_ref_note(&safe_key);

    let mut escaped = 0;
    for (message_index, content_text) in &tool_contents {
        let spans = scan_schema_ref_tokens(content_text);
        if spans.is_empty() {
            continue;
        }

        let sanitized = replace_spans(content_text, &spans, &safe_key);
        messages[*message_index]["content"] = Value::String(format!("{note}{sanitized}"));
        escaped += spans.len();
    }

    escaped
}

fn merge_request_params(
    request_params: &mut Option<HashMap<String, Value>>,
    params: HashMap<String, Value>,
) {
    request_params
        .get_or_insert_with(HashMap::new)
        .extend(params);
}

fn merge_openrouter_parameters(model: &mut ModelConfig, params: HashMap<String, Value>) {
    merge_request_params(&mut model.request_params, params);
}

impl crate::base::ProviderDescriptor for OpenRouterProvider {
    fn metadata() -> ProviderMetadata {
        ProviderMetadata::new(
            OPENROUTER_PROVIDER_NAME,
            "OpenRouter",
            "Router for many model providers",
            OPENROUTER_DEFAULT_MODEL,
            OPENROUTER_KNOWN_MODELS.to_vec(),
            OPENROUTER_DOC_URL,
            vec![
                ConfigKey::new("OPENROUTER_API_KEY", true, true, None, true),
                ConfigKey::new(
                    "OPENROUTER_HOST",
                    false,
                    false,
                    Some("https://openrouter.ai"),
                    false,
                ),
                ConfigKey::new(OPENROUTER_PARAMETERS_CONFIG_KEY, false, false, None, false),
            ],
        )
    }
}

#[async_trait]
impl Provider for OpenRouterProvider {
    fn get_name(&self) -> &str {
        &self.name
    }

    fn skip_canonical_filtering(&self) -> bool {
        true
    }

    async fn fetch_recommended_models(&self, toolshim: bool) -> Result<Vec<String>, ProviderError> {
        let response = self
            .api_client
            .request("api/v1/models")
            .response_get()
            .await
            .map_err(|e| {
                ProviderError::RequestFailed(format!(
                    "Failed to fetch models from OpenRouter API: {}",
                    e
                ))
            })?;

        let json: serde_json::Value = response.json().await.map_err(|e| {
            ProviderError::RequestFailed(format!(
                "Failed to parse OpenRouter API response as JSON: {}",
                e
            ))
        })?;

        if let Some(err_obj) = json.get("error") {
            let msg = err_obj
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error");
            return Err(ProviderError::RequestFailed(format!(
                "OpenRouter API returned an error: {}",
                msg
            )));
        }

        let data = json.get("data").and_then(|v| v.as_array()).ok_or_else(|| {
            ProviderError::UsageError("Missing data field in JSON response".into())
        })?;

        let mut models: Vec<String> = data
            .iter()
            .filter_map(|model| {
                let id = model.get("id").and_then(|v| v.as_str())?;
                if toolshim {
                    return Some(id.to_string());
                }
                let supports_tools = model
                    .get("supported_parameters")
                    .and_then(|v| v.as_array())
                    .is_some_and(|params| params.iter().any(|p| p.as_str() == Some("tools")));
                if supports_tools {
                    Some(id.to_string())
                } else {
                    None
                }
            })
            .collect();
        models.sort();
        Ok(models)
    }

    async fn stream(
        &self,
        model_config: &ModelConfig,
        system: &str,
        messages: &[Message],
        tools: &[Tool],
    ) -> Result<MessageStream, ProviderError> {
        let session_id = self
            .session_id_provider
            .as_ref()
            .and_then(|provider| provider())
            .unwrap_or_default();

        let mut merged_model;
        let model_config = if let Some(params) = &self.configured_parameters {
            merged_model = model_config.clone();
            merge_openrouter_parameters(&mut merged_model, params.clone());
            &merged_model
        } else {
            model_config
        };

        let mut payload = create_request(
            model_config,
            system,
            messages,
            tools,
            &ImageFormat::OpenAi,
            true,
        )?;

        if !session_id.is_empty() {
            if let Some(obj) = payload.as_object_mut() {
                obj.insert("user".to_string(), Value::String(session_id.to_string()));
                obj.insert(
                    "session_id".to_string(),
                    Value::String(session_id.to_string()),
                );
            }
        }

        if CacheSemantics::for_model(OPENROUTER_PROVIDER_NAME, &model_config.model_name)
            .uses_explicit_breakpoints()
            && !model_config.prompt_cache_disabled()
        {
            apply_chat_payload_breakpoints(&mut payload);
        }

        apply_gemini_compatibility(&model_config.model_name, &mut payload, messages);
        let sent_reasoning_disable =
            openrouter_format::apply_reasoning_config(&mut payload, model_config);

        if let Some(obj) = payload.as_object_mut() {
            obj.insert("transforms".to_string(), json!(["middle-out"]));
            obj.insert("usage".to_string(), json!({ "include": true }));
        }

        let mut log = start_log(model_config, &payload)?;

        let response = match self.post_chat_completions(model_config, &payload).await {
            // Mandatory-reasoning endpoints reject the disable request, so
            // downgrade to the lowest effort they all accept and retry once.
            Err(error) if sent_reasoning_disable && is_mandatory_reasoning_error(&error) => {
                let _ = log.error(&error);
                payload["reasoning"] = json!({ "effort": "low" });
                log = start_log(model_config, &payload)?;
                self.post_chat_completions(model_config, &payload).await
            }
            result => result,
        }
        .inspect_err(|e| {
            let _ = log.error(e);
        })?;

        stream_openai_compat(response, log)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::ProviderDescriptor;

    fn model_config(model_name: &str) -> ModelConfig {
        ModelConfig {
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
        }
    }

    #[test]
    fn metadata_includes_openrouter_parameters_config_key() {
        let metadata = OpenRouterProvider::metadata();

        assert!(metadata
            .config_keys
            .iter()
            .any(|key| key.name == OPENROUTER_PARAMETERS_CONFIG_KEY));
    }

    #[test]
    fn merge_openrouter_parameters_updates_model_request_params() {
        let mut model = model_config("anthropic/claude-sonnet-4");
        model.request_params = Some(HashMap::from([("verbosity".to_string(), json!("low"))]));
        let params = HashMap::from([
            ("plugins".to_string(), json!([{ "id": "web" }])),
            ("verbosity".to_string(), json!("xhigh")),
        ]);

        merge_openrouter_parameters(&mut model, params);

        let request_params = model.request_params.as_ref().unwrap();
        assert_eq!(request_params["plugins"], json!([{ "id": "web" }]));
        assert_eq!(request_params["verbosity"], json!("xhigh"));
    }

    #[tokio::test]
    async fn stream_downgrades_reasoning_disable_on_mandatory_endpoint() {
        use wiremock::matchers::{body_partial_json, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/chat/completions"))
            .and(body_partial_json(
                json!({ "reasoning": { "enabled": false } }),
            ))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({
                "error": { "message": "Reasoning is mandatory for this endpoint and cannot be disabled." }
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/chat/completions"))
            .and(body_partial_json(
                json!({ "reasoning": { "effort": "low" } }),
            ))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string("data: [DONE]\n\n"),
            )
            .expect(1)
            .mount(&server)
            .await;

        let provider = OpenRouterProvider::new(
            ApiClient::new_with_tls(
                server.uri(),
                crate::api_client::AuthMethod::BearerToken("test-key".to_string()),
                None,
            )
            .unwrap(),
            None,
            None,
        );

        let mut config = model_config("google/gemini-3.5-flash");
        config.reasoning = Some(true);
        config.request_params = Some(HashMap::from([(
            "thinking_effort".to_string(),
            json!("off"),
        )]));

        let _stream = provider
            .stream(&config, "system", &[Message::user().with_text("hi")], &[])
            .await
            .unwrap();
    }

    #[test]
    fn gemini_tool_result_schema_ref_keys_are_escaped_reversibly() {
        let mut payload = json!({
            "messages": [
                {
                    "role": "assistant",
                    "content": "Keep {'$ref': '#/components/schemas/AssistantText'} unchanged"
                },
                {
                    "role": "tool",
                    "tool_call_id": "call_1",
                    "content": "{\"$ref\": \"#/components/schemas/Usage\", \"nested\": {\"$ref\" : \"#/components/schemas/Base64Image\"}, \"items\": [{\"$ref\": \"#/components/schemas/Item\"}], \"description\": \"use $ref here\", \"literal\": \"$ref\", \"identifier\": \"$reference\"}"
                }
            ]
        });

        assert_eq!(
            escape_gemini_schema_ref_keys_in_tool_responses(&mut payload),
            5
        );
        assert_eq!(
            payload["messages"][0]["content"],
            "Keep {'$ref': '#/components/schemas/AssistantText'} unchanged"
        );
        assert_eq!(
            payload["messages"][1]["content"],
            format!(
                "{}{{\"dollar_ref\": \"#/components/schemas/Usage\", \"nested\": {{\"dollar_ref\" : \"#/components/schemas/Base64Image\"}}, \"items\": [{{\"dollar_ref\": \"#/components/schemas/Item\"}}], \"description\": \"use dollar_ref here\", \"literal\": \"dollar_ref\", \"identifier\": \"$reference\"}}",
                gemini_schema_ref_note("dollar_ref")
            )
        );
    }

    #[test]
    fn gemini_schema_ref_escape_rewrites_issue_11260_reproduction() {
        let reproduction = "{'properties': {'image': {'$ref': '#/components/schemas/Example'}}}";
        let mut payload = json!({
            "messages": [{
                "role": "tool",
                "tool_call_id": "call_1",
                "content": reproduction
            }]
        });

        assert_eq!(
            escape_gemini_schema_ref_keys_in_tool_responses(&mut payload),
            1
        );
        assert_eq!(
            payload["messages"][0]["content"],
            format!(
                "{}{{'properties': {{'image': {{'dollar_ref': '#/components/schemas/Example'}}}}}}",
                gemini_schema_ref_note("dollar_ref")
            )
        );
    }

    #[test]
    fn gemini_schema_ref_escape_preserves_longer_identifiers() {
        let untouched = "$refs and $refresh_token and $reference stay intact";
        let mut payload = json!({
            "messages": [{
                "role": "tool",
                "tool_call_id": "call_1",
                "content": untouched
            }]
        });
        let original = payload.clone();

        assert_eq!(
            escape_gemini_schema_ref_keys_in_tool_responses(&mut payload),
            0
        );
        assert_eq!(payload, original);
    }

    #[test]
    fn gemini_schema_ref_escape_preserves_literal_escaped_unicode_text() {
        let original = r#"{"literal":"\\u0024ref"}"#;
        let mut payload = json!({
            "messages": [{
                "role": "tool",
                "tool_call_id": "call_1",
                "content": original
            }]
        });
        let untouched = payload.clone();

        assert_eq!(
            escape_gemini_schema_ref_keys_in_tool_responses(&mut payload),
            0
        );
        assert_eq!(payload, untouched);
    }

    #[test]
    fn gemini_schema_ref_escape_rewrites_unicode_token_after_even_backslashes() {
        let mut payload = json!({
            "messages": [{
                "role": "tool",
                "tool_call_id": "call_1",
                "content": r#"{"a":"x\\\\","\u0024ref":"A"}"#
            }]
        });

        assert_eq!(
            escape_gemini_schema_ref_keys_in_tool_responses(&mut payload),
            1
        );
        assert!(payload["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains(r#""dollar_ref":"A""#));
    }

    #[test]
    fn gemini_schema_ref_escape_matches_partially_escaped_keys() {
        let mut payload = json!({
            "messages": [{
                "role": "tool",
                "tool_call_id": "call_1",
                "content": r#"{"$\u0072ef":"A"}"#
            }]
        });

        assert_eq!(
            escape_gemini_schema_ref_keys_in_tool_responses(&mut payload),
            1
        );
        assert!(payload["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains(r#""dollar_ref":"A""#));
    }

    #[test]
    fn gemini_schema_ref_escape_matches_fully_escaped_keys() {
        let mut payload = json!({
            "messages": [{
                "role": "tool",
                "tool_call_id": "call_1",
                "content": r#"{"\u0024\u0072\u0065\u0066":"A"}"#
            }]
        });

        assert_eq!(
            escape_gemini_schema_ref_keys_in_tool_responses(&mut payload),
            1
        );
        assert!(payload["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains(r#""dollar_ref":"A""#));
    }

    #[test]
    fn gemini_schema_ref_escape_preserves_escaped_longer_identifiers() {
        let original = r#"{"$\u0072efresh_token":"A"}"#;
        let mut payload = json!({
            "messages": [{
                "role": "tool",
                "tool_call_id": "call_1",
                "content": original
            }]
        });
        let untouched = payload.clone();

        assert_eq!(
            escape_gemini_schema_ref_keys_in_tool_responses(&mut payload),
            0
        );
        assert_eq!(payload, untouched);
    }

    #[test]
    fn gemini_compatibility_only_applies_to_gemini_models() {
        let tool_result = r##"{"$ref":"#/components/schemas/Usage"}"##;
        let mut gemma_payload = json!({
            "messages": [{
                "role": "tool",
                "tool_call_id": "call_1",
                "content": tool_result
            }]
        });
        let original_gemma_payload = gemma_payload.clone();

        apply_gemini_compatibility("google/gemma-3-27b-it", &mut gemma_payload, &[]);

        assert_eq!(gemma_payload, original_gemma_payload);

        let mut anthropic_payload = original_gemma_payload.clone();
        apply_gemini_compatibility("anthropic/claude-sonnet-4.5", &mut anthropic_payload, &[]);

        assert_eq!(anthropic_payload, original_gemma_payload);

        let mut gemini_payload = original_gemma_payload;
        apply_gemini_compatibility("google/gemini-2.5-flash", &mut gemini_payload, &[]);

        assert_eq!(
            gemini_payload["messages"][0]["content"],
            format!(
                "{}{{\"dollar_ref\":\"#/components/schemas/Usage\"}}",
                gemini_schema_ref_note("dollar_ref")
            )
        );
        assert!(is_gemini_model("google/gemini-2.0-flash-exp:free"));
        assert!(!is_gemini_model("google/gemma-3-27b-it"));
    }

    #[test]
    fn gemini_schema_ref_escape_rewrites_value_position_tokens() {
        let mut payload = json!({
            "messages": [{
                "role": "tool",
                "tool_call_id": "call_1",
                "content": "{\"description\":\"use $ref here\",\"literal\":\"$ref\",\"identifier\":\"$reference\"}"
            }]
        });

        assert_eq!(
            escape_gemini_schema_ref_keys_in_tool_responses(&mut payload),
            2
        );
        assert_eq!(
            payload["messages"][0]["content"],
            format!(
                "{}{{\"description\":\"use dollar_ref here\",\"literal\":\"dollar_ref\",\"identifier\":\"$reference\"}}",
                gemini_schema_ref_note("dollar_ref")
            )
        );
    }

    #[test]
    fn gemini_schema_ref_escape_rewrites_non_json_prose() {
        let mut payload = json!({
            "messages": [{
                "role": "tool",
                "tool_call_id": "call_1",
                "content": "log entry, \"$ref\": not a JSON key"
            }]
        });

        assert_eq!(
            escape_gemini_schema_ref_keys_in_tool_responses(&mut payload),
            1
        );
        assert_eq!(
            payload["messages"][0]["content"],
            format!(
                "{}log entry, \"dollar_ref\": not a JSON key",
                gemini_schema_ref_note("dollar_ref")
            )
        );
    }

    #[test]
    fn gemini_schema_ref_escape_preserves_duplicate_ref_members() {
        let mut payload = json!({
            "messages": [{
                "role": "tool",
                "tool_call_id": "call_1",
                "content": "{\"$ref\":\"A\",\"$ref\":\"B\"}"
            }]
        });

        assert_eq!(
            escape_gemini_schema_ref_keys_in_tool_responses(&mut payload),
            2
        );
        assert_eq!(
            payload["messages"][0]["content"],
            format!(
                "{}{{\"dollar_ref\":\"A\",\"dollar_ref\":\"B\"}}",
                gemini_schema_ref_note("dollar_ref")
            )
        );
    }

    #[test]
    fn gemini_schema_ref_escape_preserves_unrelated_duplicate_members() {
        let mut payload = json!({
            "messages": [{
                "role": "tool",
                "tool_call_id": "call_1",
                "content": "{\"duplicate\":\"first\",\"$ref\":\"A\",\"duplicate\":\"second\"}"
            }]
        });

        assert_eq!(
            escape_gemini_schema_ref_keys_in_tool_responses(&mut payload),
            1
        );
        assert_eq!(
            payload["messages"][0]["content"],
            format!(
                "{}{{\"duplicate\":\"first\",\"dollar_ref\":\"A\",\"duplicate\":\"second\"}}",
                gemini_schema_ref_note("dollar_ref")
            )
        );
    }

    #[test]
    fn gemini_schema_ref_escape_rewrites_json_embedded_in_prose() {
        let mut payload = json!({
            "messages": [{
                "role": "tool",
                "tool_call_id": "call_1",
                "content": "before output { \"$ref\" : \"A\" } after output"
            }]
        });

        assert_eq!(
            escape_gemini_schema_ref_keys_in_tool_responses(&mut payload),
            1
        );
        assert_eq!(
            payload["messages"][0]["content"],
            format!(
                "{}before output {{ \"dollar_ref\" : \"A\" }} after output",
                gemini_schema_ref_note("dollar_ref")
            )
        );
    }

    #[test]
    fn gemini_schema_ref_escape_rewrites_multiple_occurrences_in_one_result() {
        let mut payload = json!({
            "messages": [{
                "role": "tool",
                "tool_call_id": "call_1",
                "content": "{\"$ref\":\"A\",\"nested\":{\"$ref\":\"B\"}}"
            }]
        });

        assert_eq!(
            escape_gemini_schema_ref_keys_in_tool_responses(&mut payload),
            2
        );
        assert_eq!(
            payload["messages"][0]["content"],
            format!(
                "{}{{\"dollar_ref\":\"A\",\"nested\":{{\"dollar_ref\":\"B\"}}}}",
                gemini_schema_ref_note("dollar_ref")
            )
        );
    }

    #[test]
    fn gemini_schema_ref_escape_matches_decoded_keys_and_collisions() {
        let mut payload = json!({
            "messages": [{
                "role": "tool",
                "tool_call_id": "call_1",
                "content": r#"{"\u0024ref":"A","\u0064ollar_ref":"B"}"#
            }]
        });

        assert_eq!(
            escape_gemini_schema_ref_keys_in_tool_responses(&mut payload),
            1
        );
        assert_eq!(
            payload["messages"][0]["content"],
            format!(
                r#"{}{{"dollar_ref_2":"A","\u0064ollar_ref":"B"}}"#,
                gemini_schema_ref_note("dollar_ref_2")
            )
        );
    }

    #[test]
    fn gemini_schema_ref_escape_honors_escaped_quotes_and_backslashes() {
        let original_content =
            r#"{"text":"escaped quote \" then \\ and \"$ref\": still text","$ref":"A"}"#;
        let mut payload = json!({
            "messages": [{
                "role": "tool",
                "tool_call_id": "call_1",
                "content": original_content
            }]
        });

        assert_eq!(
            escape_gemini_schema_ref_keys_in_tool_responses(&mut payload),
            2
        );
        assert_eq!(
            payload["messages"][0]["content"],
            format!(
                r#"{}{{"text":"escaped quote \" then \\ and \"dollar_ref\": still text","dollar_ref":"A"}}"#,
                gemini_schema_ref_note("dollar_ref")
            )
        );
    }

    #[test]
    fn gemini_schema_ref_escape_preserves_absent_content_bytes() {
        let original_content = "prefix { \"ordinary\" : [ 1, 2 ] } suffix";
        let mut payload = json!({
            "messages": [{
                "role": "tool",
                "tool_call_id": "call_1",
                "content": original_content
            }]
        });

        assert_eq!(
            escape_gemini_schema_ref_keys_in_tool_responses(&mut payload),
            0
        );
        assert_eq!(
            payload["messages"][0]["content"]
                .as_str()
                .unwrap()
                .as_bytes(),
            original_content.as_bytes()
        );
    }

    #[test]
    fn gemini_schema_ref_escape_tolerates_malformed_json() {
        let mut payload = json!({
            "messages": [
                {
                    "role": "tool",
                    "tool_call_id": "call_1",
                    "content": "{\"$ref\": \"A\""
                },
                {
                    "role": "tool",
                    "tool_call_id": "call_2",
                    "content": "}"
                }
            ]
        });

        assert_eq!(
            escape_gemini_schema_ref_keys_in_tool_responses(&mut payload),
            1
        );
        assert_eq!(
            payload["messages"][0]["content"],
            format!(
                "{}{{\"dollar_ref\": \"A\"",
                gemini_schema_ref_note("dollar_ref")
            )
        );
        assert_eq!(payload["messages"][1]["content"], "}");
    }

    #[test]
    fn gemini_schema_ref_escape_avoids_existing_safe_key() {
        let mut payload = json!({
            "messages": [{
                "role": "tool",
                "tool_call_id": "call_1",
                "content": "{\"$ref\":\"A\",\"dollar_ref\":\"B\"}"
            }]
        });

        assert_eq!(
            escape_gemini_schema_ref_keys_in_tool_responses(&mut payload),
            1
        );
        assert_eq!(
            payload["messages"][0]["content"],
            format!(
                "{}{{\"dollar_ref_2\":\"A\",\"dollar_ref\":\"B\"}}",
                gemini_schema_ref_note("dollar_ref_2")
            )
        );
    }

    #[test]
    fn gemini_schema_ref_escape_advances_past_multiple_key_collisions() {
        let mut payload = json!({
            "messages": [{
                "role": "tool",
                "tool_call_id": "call_1",
                "content": "{\"$ref\":\"A\",\"dollar_ref\":\"B\",\"dollar_ref_2\":\"C\"}"
            }]
        });

        assert_eq!(
            escape_gemini_schema_ref_keys_in_tool_responses(&mut payload),
            1
        );
        assert_eq!(
            payload["messages"][0]["content"],
            format!(
                "{}{{\"dollar_ref_3\":\"A\",\"dollar_ref\":\"B\",\"dollar_ref_2\":\"C\"}}",
                gemini_schema_ref_note("dollar_ref_3")
            )
        );
    }

    #[test]
    fn gemini_schema_ref_escape_advances_past_deep_collision_ladder() {
        let mut payload = json!({
            "messages": [
                {
                    "role": "tool",
                    "tool_call_id": "call_1",
                    "content": "{\"$ref\":\"A\",\"dollar_ref\":\"B\",\"nested\":{\"dollar_ref_2\":\"C\",\"items\":[{\"dollar_ref_3\":\"D\"},{\"dollar_ref_4\":\"E\"}]}}"
                },
                {
                    "role": "tool",
                    "tool_call_id": "call_2",
                    "content": "{\"dollar_ref_5\":\"F\"}"
                }
            ]
        });

        assert_eq!(
            escape_gemini_schema_ref_keys_in_tool_responses(&mut payload),
            1
        );
        assert_eq!(
            payload["messages"][0]["content"],
            format!(
                "{}{{\"dollar_ref_6\":\"A\",\"dollar_ref\":\"B\",\"nested\":{{\"dollar_ref_2\":\"C\",\"items\":[{{\"dollar_ref_3\":\"D\"}},{{\"dollar_ref_4\":\"E\"}}]}}}}",
                gemini_schema_ref_note("dollar_ref_6")
            )
        );
        assert_eq!(
            payload["messages"][1]["content"],
            "{\"dollar_ref_5\":\"F\"}"
        );
    }

    #[test]
    fn gemini_schema_ref_escape_ignores_non_tool_content_and_safe_tool_results() {
        let mut payload = json!({
            "messages": [
                { "role": "user", "content": "{\"$ref\":\"#/components/schemas/UserText\"}" },
                { "role": "assistant", "content": "{\"$ref\":\"#/components/schemas/AssistantText\"}" },
                { "role": "tool", "tool_call_id": "call_1", "content": "ordinary output" },
                { "role": "tool", "tool_call_id": "call_2", "content": [{ "type": "text", "text": "{'$ref': '#/components/schemas/Structured'}" }] }
            ]
        });
        let original = payload.clone();

        assert_eq!(
            escape_gemini_schema_ref_keys_in_tool_responses(&mut payload),
            0
        );
        assert_eq!(payload, original);
    }

    #[test]
    fn gemini_schema_ref_escape_ignores_braces_inside_string_literals() {
        let mut payload = json!({
            "messages": [{
                "role": "tool",
                "tool_call_id": "call_1",
                "content": r#"{"a":"}}}}","$ref":"B"}"#
            }]
        });

        assert_eq!(
            escape_gemini_schema_ref_keys_in_tool_responses(&mut payload),
            1
        );
        assert_eq!(
            payload["messages"][0]["content"],
            Value::String(
                gemini_schema_ref_note("dollar_ref") + r#"{"a":"}}}}","dollar_ref":"B"}"#
            )
        );
    }

    #[test]
    fn gemini_schema_ref_escape_ignores_brackets_inside_string_literals() {
        let mut payload = json!({
            "messages": [{
                "role": "tool",
                "tool_call_id": "call_1",
                "content": r#"{"a":"[[[","$ref":"B"}"#
            }]
        });

        assert_eq!(
            escape_gemini_schema_ref_keys_in_tool_responses(&mut payload),
            1
        );
        assert_eq!(
            payload["messages"][0]["content"],
            Value::String(gemini_schema_ref_note("dollar_ref") + r#"{"a":"[[[","dollar_ref":"B"}"#)
        );
    }

    #[test]
    fn gemini_schema_ref_escape_avoids_safe_key_used_in_another_tool_result() {
        let untouched = r#"{"dollar_ref":"x"}"#;
        let mut payload = json!({
            "messages": [
                {
                    "role": "tool",
                    "tool_call_id": "call_1",
                    "content": r#"{"$ref":"A"}"#
                },
                {
                    "role": "tool",
                    "tool_call_id": "call_2",
                    "content": untouched
                }
            ]
        });

        assert_eq!(
            escape_gemini_schema_ref_keys_in_tool_responses(&mut payload),
            1
        );
        assert_eq!(
            payload["messages"][0]["content"],
            Value::String(gemini_schema_ref_note("dollar_ref_2") + r#"{"dollar_ref_2":"A"}"#)
        );
        assert_eq!(
            payload["messages"][1]["content"],
            Value::String(untouched.to_string())
        );
    }

    #[test]
    fn gemini_schema_ref_escape_rewrites_nested_encoded_json() {
        let mut payload = json!({
            "messages": [{
                "role": "tool",
                "tool_call_id": "call_1",
                "content": r#"{"payload":"{\"$ref\":\"A\"}"}"#
            }]
        });

        assert_eq!(
            escape_gemini_schema_ref_keys_in_tool_responses(&mut payload),
            1
        );
        assert_eq!(
            payload["messages"][0]["content"],
            Value::String(
                gemini_schema_ref_note("dollar_ref") + r#"{"payload":"{\"dollar_ref\":\"A\"}"}"#
            )
        );
    }

    #[test]
    fn gemini_schema_ref_escape_preserves_multibyte_content() {
        let mut payload = json!({
            "messages": [{
                "role": "tool",
                "tool_call_id": "call_1",
                "content": "{\"\u{540d}\u{524d}\":\"\u{1f389} ok\",\"$ref\":\"A\"}"
            }]
        });

        assert_eq!(
            escape_gemini_schema_ref_keys_in_tool_responses(&mut payload),
            1
        );
        assert_eq!(
            payload["messages"][0]["content"],
            Value::String(
                gemini_schema_ref_note("dollar_ref")
                    + "{\"\u{540d}\u{524d}\":\"\u{1f389} ok\",\"dollar_ref\":\"A\"}"
            )
        );
    }

    #[test]
    fn gemini_schema_ref_escape_is_idempotent() {
        let mut payload = json!({
            "messages": [{
                "role": "tool",
                "tool_call_id": "call_1",
                "content": r#"{"$ref":"A","b":[{"$ref":"B"}]}"#
            }]
        });

        assert_eq!(
            escape_gemini_schema_ref_keys_in_tool_responses(&mut payload),
            2
        );
        let after_first_pass = payload.clone();
        assert_eq!(
            escape_gemini_schema_ref_keys_in_tool_responses(&mut payload),
            0
        );
        assert_eq!(payload, after_first_pass);
    }
}
