use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

#[derive(Clone, Copy)]
pub(super) struct ProviderFeatures {
    pub(super) reports_usage: bool,
    pub(super) preserves_thinking: bool,
    pub(super) resolved_model: Option<&'static str>,
    pub(super) cache_read_tokens: Option<i32>,
    pub(super) cache_write_tokens: Option<i32>,
    pub(super) manages_own_context: bool,
    /// Serves this limit from `get_context_limit` for the session's model, as
    /// a declarative provider's configured model would. The compaction
    /// operation itself keeps using the model config's limit.
    pub(super) context_limit_override: Option<usize>,
}

impl Default for ProviderFeatures {
    fn default() -> Self {
        Self {
            reports_usage: true,
            preserves_thinking: false,
            resolved_model: None,
            cache_read_tokens: None,
            cache_write_tokens: None,
            manages_own_context: false,
            context_limit_override: None,
        }
    }
}

#[derive(Clone)]
enum ApiResponse {
    Reply(String),
    ReplyWithDistinctIds(Vec<String>),
    ToolCall {
        name: String,
        arguments: String,
        require_advertised: bool,
    },
    ToolCalls(Vec<ApiToolCall>),
    Mixed {
        reasoning: String,
        text: Option<String>,
        call: Option<ApiToolCall>,
    },
    NoChoices,
    OutputLimit,
    ContextLimitError(String),
    ServerError(String),
    EmptyServerError,
    ReplyThenServerError {
        reply: String,
        error: String,
    },
}

#[derive(Clone)]
struct ApiToolCall {
    id: String,
    name: String,
    arguments: String,
}

struct ApiRule {
    matcher: ApiMatcher,
    response: ApiResponse,
    gate: Option<ResponseGate>,
}

enum ApiMatcher {
    InputContains(String),
    SystemContains(String),
}

struct DummyApiState {
    features: ProviderFeatures,
    rules: Mutex<Vec<ApiRule>>,
    calls: Mutex<Vec<ApiCall>>,
    next_response_id: AtomicUsize,
    context_limit_rejections: AtomicUsize,
    /// Rejection wall for every request, replacing the canonical limit of the
    /// request's model. Zero keeps the canonical lookup.
    context_limit_override: AtomicUsize,
}

pub(super) struct DummyApi {
    server: MockServer,
    state: Arc<DummyApiState>,
}

#[derive(Clone)]
pub(super) struct ResponseGate {
    state: Arc<(Mutex<GateState>, Condvar)>,
}

#[derive(Default)]
struct GateState {
    entered: bool,
    released: bool,
}

impl ResponseGate {
    fn new() -> Self {
        Self {
            state: Arc::new((Mutex::new(GateState::default()), Condvar::new())),
        }
    }

    fn block(&self) {
        let (state, changed) = &*self.state;
        let mut state = state.lock().unwrap();
        state.entered = true;
        changed.notify_all();
        while !state.released {
            state = changed.wait(state).unwrap();
        }
    }

    pub(super) async fn entered(&self) {
        let gate = self.clone();
        tokio::task::spawn_blocking(move || {
            let (state, changed) = &*gate.state;
            let mut state = state.lock().unwrap();
            while !state.entered {
                state = changed.wait(state).unwrap();
            }
        })
        .await
        .unwrap();
    }

    pub(super) fn release(&self) {
        let (state, changed) = &*self.state;
        state.lock().unwrap().released = true;
        changed.notify_all();
    }
}

#[derive(Clone)]
pub(super) struct ApiCall {
    body: Value,
}

impl ApiCall {
    pub(super) fn input_tokens(&self) -> i32 {
        serialized_chars(&self.body)
    }

    pub(super) fn input_contains(&self, needle: &str) -> bool {
        request_input(&self.body).contains(needle)
    }

    pub(super) fn uses_model(&self, model: &str) -> bool {
        self.body["model"].as_str() == Some(model)
    }

    pub(super) fn input_occurrences(&self, needle: &str) -> usize {
        request_input(&self.body).matches(needle).count()
    }

    pub(super) fn system_contains(&self, needle: &str) -> bool {
        request_system(&self.body).contains(needle)
    }

    pub(super) fn advertises_tool(&self, name: &str) -> bool {
        self.body["tools"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|tool| tool["function"]["name"].as_str() == Some(name))
    }

    pub(super) fn tool_schema(&self, name: &str) -> Option<&Value> {
        self.body["tools"]
            .as_array()?
            .iter()
            .find(|tool| tool["function"]["name"].as_str() == Some(name))
            .map(|tool| &tool["function"]["parameters"])
    }

    pub(super) fn input_has_image(&self, mime_type: &str, data: &str) -> bool {
        let expected = format!("data:{mime_type};base64,{data}");
        self.body["messages"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|message| message["content"].as_array().into_iter().flatten())
            .any(|content| content["image_url"]["url"].as_str() == Some(expected.as_str()))
    }
}

impl DummyApi {
    pub(super) async fn start(features: ProviderFeatures) -> Self {
        let server = MockServer::start().await;
        let state = Arc::new(DummyApiState {
            features,
            rules: Mutex::new(Vec::new()),
            calls: Mutex::new(Vec::new()),
            next_response_id: AtomicUsize::new(1),
            context_limit_rejections: AtomicUsize::new(0),
            context_limit_override: AtomicUsize::new(0),
        });
        let responder = state.clone();
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(move |request: &Request| responder.respond(request))
            .mount(&server)
            .await;
        Self { server, state }
    }

    pub(super) fn uri(&self) -> String {
        self.server.uri()
    }

    pub(super) fn on(&self, needle: impl Into<String>) -> ApiRuleBuilder<'_> {
        ApiRuleBuilder {
            api: self,
            matcher: ApiMatcher::InputContains(needle.into()),
        }
    }

    pub(super) fn on_system(&self, needle: impl Into<String>) -> ApiRuleBuilder<'_> {
        ApiRuleBuilder {
            api: self,
            matcher: ApiMatcher::SystemContains(needle.into()),
        }
    }

    pub(super) fn calls(&self) -> Vec<ApiCall> {
        self.state.calls.lock().unwrap().clone()
    }

    pub(super) fn call_count(&self) -> usize {
        self.state.calls.lock().unwrap().len()
    }

    /// Requests the API rejected with a context-length 400, whether enforced
    /// from the model's canonical limit or scripted via `context_limit_error`.
    pub(super) fn context_limit_rejections(&self) -> usize {
        self.state.context_limit_rejections.load(Ordering::SeqCst)
    }

    /// Pins the rejection wall for every request, decoupling it from the
    /// canonical limit of the request's model (which thresholds derived from
    /// the provider still use). Lets a test grow a context far past its
    /// threshold without tripping the wall.
    pub(super) fn set_context_limit(&self, limit: usize) {
        self.state
            .context_limit_override
            .store(limit, Ordering::SeqCst);
    }

    fn add_rule(&self, matcher: ApiMatcher, response: ApiResponse) -> usize {
        let mut rules = self.state.rules.lock().unwrap();
        rules.push(ApiRule {
            matcher,
            response,
            gate: None,
        });
        rules.len() - 1
    }

    fn add_gated_rule(&self, matcher: ApiMatcher, response: ApiResponse, gate: ResponseGate) {
        self.state.rules.lock().unwrap().push(ApiRule {
            matcher,
            response,
            gate: Some(gate),
        });
    }
}

pub(super) struct ApiRuleBuilder<'a> {
    api: &'a DummyApi,
    matcher: ApiMatcher,
}

pub(super) struct ConfiguredResponse<'a> {
    api: &'a DummyApi,
    rule: usize,
}

impl<'a> ApiRuleBuilder<'a> {
    pub(super) fn reply(self, text: impl Into<String>) -> ConfiguredResponse<'a> {
        self.configured(ApiResponse::Reply(text.into()))
    }

    pub(super) fn reply_with_distinct_ids<const N: usize>(
        self,
        chunks: [&str; N],
    ) -> ConfiguredResponse<'a> {
        self.configured(ApiResponse::ReplyWithDistinctIds(
            chunks.into_iter().map(str::to_string).collect(),
        ))
    }

    pub(super) fn hold_reply(self, text: impl Into<String>) -> ResponseGate {
        let gate = ResponseGate::new();
        self.api
            .add_gated_rule(self.matcher, ApiResponse::Reply(text.into()), gate.clone());
        gate
    }

    pub(super) fn reasoning(self, text: impl Into<String>) -> ConfiguredResponse<'a> {
        self.configured(ApiResponse::Mixed {
            reasoning: text.into(),
            text: None,
            call: None,
        })
    }

    pub(super) fn call(self, name: impl Into<String>, arguments: Value) -> ConfiguredResponse<'a> {
        self.configured(ApiResponse::ToolCall {
            name: name.into(),
            arguments: arguments.to_string(),
            require_advertised: true,
        })
    }

    pub(super) fn unadvertised_call(
        self,
        name: impl Into<String>,
        arguments: Value,
    ) -> ConfiguredResponse<'a> {
        self.configured(ApiResponse::ToolCall {
            name: name.into(),
            arguments: arguments.to_string(),
            require_advertised: false,
        })
    }

    pub(super) fn calls<const N: usize>(
        self,
        calls: [(&str, &str, Value); N],
    ) -> ConfiguredResponse<'a> {
        self.configured(ApiResponse::ToolCalls(
            calls
                .into_iter()
                .map(|(id, name, arguments)| ApiToolCall {
                    id: id.to_string(),
                    name: name.to_string(),
                    arguments: arguments.to_string(),
                })
                .collect(),
        ))
    }

    pub(super) fn malformed_tool_call(
        self,
        name: impl Into<String>,
        arguments: impl Into<String>,
    ) -> ConfiguredResponse<'a> {
        self.configured(ApiResponse::ToolCall {
            name: name.into(),
            arguments: arguments.into(),
            require_advertised: true,
        })
    }

    pub(super) fn context_limit_error(self, message: impl Into<String>) -> ConfiguredResponse<'a> {
        self.configured(ApiResponse::ContextLimitError(message.into()))
    }

    pub(super) fn server_error(self, message: impl Into<String>) -> ConfiguredResponse<'a> {
        self.configured(ApiResponse::ServerError(message.into()))
    }

    pub(super) fn no_choices(self) -> ConfiguredResponse<'a> {
        self.configured(ApiResponse::NoChoices)
    }

    pub(super) fn output_limit(self) -> ConfiguredResponse<'a> {
        self.configured(ApiResponse::OutputLimit)
    }

    pub(super) fn empty_server_error(self) -> ConfiguredResponse<'a> {
        self.configured(ApiResponse::EmptyServerError)
    }

    fn configured(self, response: ApiResponse) -> ConfiguredResponse<'a> {
        ConfiguredResponse {
            api: self.api,
            rule: self.api.add_rule(self.matcher, response),
        }
    }
}

impl<'a> ConfiguredResponse<'a> {
    pub(super) fn reply(self, text: impl Into<String>) -> Self {
        let mut rules = self.api.state.rules.lock().unwrap();
        let ApiResponse::Mixed {
            text: response_text,
            ..
        } = &mut rules[self.rule].response
        else {
            panic!("reply can only follow reasoning");
        };
        *response_text = Some(text.into());
        drop(rules);
        self
    }

    pub(super) fn call(self, name: impl Into<String>, arguments: Value) -> Self {
        let mut rules = self.api.state.rules.lock().unwrap();
        let ApiResponse::Mixed { call, .. } = &mut rules[self.rule].response else {
            panic!("call can only follow reasoning");
        };
        *call = Some(ApiToolCall {
            id: String::new(),
            name: name.into(),
            arguments: arguments.to_string(),
        });
        drop(rules);
        self
    }

    pub(super) fn malformed_call(
        self,
        name: impl Into<String>,
        arguments: impl Into<String>,
    ) -> Self {
        let mut rules = self.api.state.rules.lock().unwrap();
        let ApiResponse::Mixed { call, .. } = &mut rules[self.rule].response else {
            panic!("malformed_call can only follow reasoning");
        };
        *call = Some(ApiToolCall {
            id: String::new(),
            name: name.into(),
            arguments: arguments.into(),
        });
        drop(rules);
        self
    }

    pub(super) fn server_error(self, error: impl Into<String>) -> &'a DummyApi {
        let mut rules = self.api.state.rules.lock().unwrap();
        let response = &mut rules[self.rule].response;
        let ApiResponse::Reply(reply) = response else {
            panic!("server_error can only follow reply");
        };
        *response = ApiResponse::ReplyThenServerError {
            reply: std::mem::take(reply),
            error: error.into(),
        };
        self.api
    }
}

impl DummyApiState {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: Value = request.body_json().expect("OpenAI request body");
        self.calls
            .lock()
            .unwrap()
            .push(ApiCall { body: body.clone() });

        let input_tokens = serialized_chars(&body);
        let model = body["model"].as_str().expect("OpenAI request model");
        let override_limit = self.context_limit_override.load(Ordering::SeqCst);
        let context_limit = if override_limit > 0 {
            override_limit
        } else {
            goose_providers::model::ModelConfig::new(model)
                .with_canonical_limits("openai")
                .context_limit()
        };
        if input_tokens as usize > context_limit {
            self.context_limit_rejections.fetch_add(1, Ordering::SeqCst);
            return context_limit_response(input_tokens, context_limit);
        }

        let input = request_input(&body);
        let system = request_system(&body);
        let (response, gate) = {
            let rules = self.rules.lock().unwrap();
            let rule = rules
                .iter()
                .rev()
                .find(|rule| match &rule.matcher {
                    ApiMatcher::InputContains(needle) => input.contains(needle),
                    ApiMatcher::SystemContains(needle) => system.contains(needle),
                })
                .unwrap_or_else(|| {
                    panic!("dummy API has no rule matching input {input:?}, system {system:?}")
                });
            (rule.response.clone(), rule.gate.clone())
        };
        if let Some(gate) = gate {
            gate.block();
        }

        let id = format!(
            "chatcmpl-test-{}",
            self.next_response_id.fetch_add(1, Ordering::Relaxed)
        );
        let meta = |output_tokens: i32| ResponseMeta {
            id: &id,
            model,
            input_tokens,
            output_tokens,
            include_usage: self.features.reports_usage,
            cache_read_tokens: self.features.cache_read_tokens,
            cache_write_tokens: self.features.cache_write_tokens,
        };
        match response {
            ApiResponse::Reply(text) => sse_response(reply_events(
                &meta(text.chars().count() as i32),
                &text,
                None,
            )),
            ApiResponse::ReplyWithDistinctIds(chunks) => {
                let output_tokens: usize = chunks.iter().map(|chunk| chunk.chars().count()).sum();
                sse_response(reply_events_with_distinct_ids(
                    &meta(output_tokens as i32),
                    &chunks,
                ))
            }
            ApiResponse::ToolCall {
                name,
                arguments,
                require_advertised,
            } => {
                if require_advertised {
                    assert_tool_advertised(&body, &name);
                }
                let output_tokens = name.chars().count() as i32 + arguments.chars().count() as i32;
                sse_response(tool_call_events(&meta(output_tokens), &name, &arguments))
            }
            ApiResponse::ToolCalls(calls) => {
                for call in &calls {
                    assert_tool_advertised(&body, &call.name);
                }
                let output_tokens = calls
                    .iter()
                    .map(|call| call.name.chars().count() + call.arguments.chars().count())
                    .sum::<usize>() as i32;
                sse_response(tool_calls_events(&meta(output_tokens), &calls))
            }
            ApiResponse::Mixed {
                reasoning,
                text,
                mut call,
            } => {
                if let Some(call) = &mut call {
                    assert_tool_advertised(&body, &call.name);
                    call.id = format!(
                        "dummy-tool-call-{}",
                        id.strip_prefix("chatcmpl-test-").unwrap()
                    );
                }
                let output_tokens = reasoning.chars().count()
                    + text.as_deref().unwrap_or_default().chars().count()
                    + call
                        .as_ref()
                        .map(|call| call.name.chars().count() + call.arguments.chars().count())
                        .unwrap_or_default();
                sse_response(mixed_events(
                    &meta(output_tokens as i32),
                    &reasoning,
                    text.as_deref(),
                    call.as_ref(),
                ))
            }
            ApiResponse::NoChoices => sse_response(no_choices_events(&id, model)),
            ApiResponse::OutputLimit => sse_response(output_limit_events(&meta(0))),
            ApiResponse::ContextLimitError(message) => {
                self.context_limit_rejections.fetch_add(1, Ordering::SeqCst);
                ResponseTemplate::new(400).set_body_json(context_limit_error(format!(
                    "context_length_exceeded: {message}"
                )))
            }
            ApiResponse::ServerError(message) => {
                sse_response(format!("data: {}\n\n", api_error(message)))
            }
            ApiResponse::EmptyServerError => ResponseTemplate::new(500),
            ApiResponse::ReplyThenServerError { reply, error } => sse_response(reply_events(
                &meta(reply.chars().count() as i32),
                &reply,
                Some(&error),
            )),
        }
    }
}

fn assert_tool_advertised(body: &Value, name: &str) {
    let advertised = body["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|tool| tool["function"]["name"].as_str())
        .collect::<Vec<_>>();
    assert!(
        advertised.contains(&name),
        "dummy API cannot call unadvertised tool {name:?}; advertised: {advertised:?}"
    );
}

fn serialized_chars(value: &Value) -> i32 {
    value.to_string().chars().count() as i32
}

fn context_limit_response(input_tokens: i32, context_limit: usize) -> ResponseTemplate {
    ResponseTemplate::new(400).set_body_json(context_limit_error(format!(
        "This model's maximum context length is {context_limit} tokens, but the request contains {input_tokens} tokens"
    )))
}

fn context_limit_error(message: impl Into<String>) -> Value {
    json!({
        "error": {
            "message": message.into(),
            "type": "invalid_request_error",
            "code": "context_length_exceeded"
        }
    })
}

fn request_input(body: &Value) -> String {
    let mut values = Vec::new();
    for message in body["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|message| message["role"] != "system")
    {
        collect_strings(message, &mut values);
    }
    values.join("\n")
}

fn request_system(body: &Value) -> String {
    let mut values = Vec::new();
    for message in body["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|message| message["role"] == "system")
    {
        collect_strings(message, &mut values);
    }
    values.join("\n")
}

fn collect_strings<'a>(value: &'a Value, strings: &mut Vec<&'a str>) {
    match value {
        Value::String(value) => strings.push(value),
        Value::Array(values) => {
            for value in values {
                collect_strings(value, strings);
            }
        }
        Value::Object(values) => {
            for value in values.values() {
                collect_strings(value, strings);
            }
        }
        _ => {}
    }
}

struct ResponseMeta<'a> {
    id: &'a str,
    model: &'a str,
    input_tokens: i32,
    output_tokens: i32,
    include_usage: bool,
    cache_read_tokens: Option<i32>,
    cache_write_tokens: Option<i32>,
}

fn reply_events(meta: &ResponseMeta, text: &str, error: Option<&str>) -> String {
    let ResponseMeta { id, model, .. } = meta;
    let mut events = String::new();
    for chunk in split_reply(text) {
        push_event(
            &mut events,
            json!({
                "id": id,
                "object": "chat.completion.chunk",
                "model": model,
                "choices": [{
                    "index": 0,
                    "delta": { "content": chunk },
                    "finish_reason": null
                }]
            }),
        );
    }
    push_event(
        &mut events,
        json!({
            "id": id,
            "object": "chat.completion.chunk",
            "model": model,
            "choices": [{
                "index": 0,
                "delta": {},
                "finish_reason": "stop"
            }]
        }),
    );
    if meta.include_usage {
        push_event(&mut events, usage_event(meta));
    }
    if let Some(error) = error {
        push_event(&mut events, api_error(error));
    } else {
        events.push_str("data: [DONE]\n\n");
    }
    events
}

fn reply_events_with_distinct_ids(meta: &ResponseMeta, chunks: &[String]) -> String {
    let mut events = String::new();
    for (index, chunk) in chunks.iter().enumerate() {
        push_event(
            &mut events,
            json!({
                "id": format!("{}-{index}", meta.id),
                "object": "chat.completion.chunk",
                "model": meta.model,
                "choices": [{
                    "index": 0,
                    "delta": { "content": chunk },
                    "finish_reason": null
                }]
            }),
        );
    }
    push_event(
        &mut events,
        json!({
            "id": format!("{}-{}", meta.id, chunks.len().saturating_sub(1)),
            "object": "chat.completion.chunk",
            "model": meta.model,
            "choices": [{
                "index": 0,
                "delta": {},
                "finish_reason": "stop"
            }]
        }),
    );
    if meta.include_usage {
        push_event(&mut events, usage_event(meta));
    }
    events.push_str("data: [DONE]\n\n");
    events
}

fn no_choices_events(id: &str, model: &str) -> String {
    let mut events = String::new();
    push_event(
        &mut events,
        json!({
            "id": id,
            "object": "chat.completion.chunk",
            "model": model,
            "choices": []
        }),
    );
    events.push_str("data: [DONE]\n\n");
    events
}

fn output_limit_events(meta: &ResponseMeta) -> String {
    let mut events = String::new();
    push_event(
        &mut events,
        json!({
            "id": meta.id,
            "object": "chat.completion.chunk",
            "model": meta.model,
            "choices": [{
                "index": 0,
                "delta": {},
                "finish_reason": "length"
            }]
        }),
    );
    if meta.include_usage {
        push_event(&mut events, usage_event(meta));
    }
    events.push_str("data: [DONE]\n\n");
    events
}

fn tool_call_events(meta: &ResponseMeta, name: &str, arguments: &str) -> String {
    let tool_call_id = format!(
        "dummy-tool-call-{}",
        meta.id.strip_prefix("chatcmpl-test-").unwrap()
    );
    tool_calls_events(
        meta,
        &[ApiToolCall {
            id: tool_call_id,
            name: name.to_string(),
            arguments: arguments.to_string(),
        }],
    )
}

fn tool_calls_events(meta: &ResponseMeta, calls: &[ApiToolCall]) -> String {
    let mut events = String::new();
    push_tool_call_events(&mut events, meta.id, meta.model, calls, None);
    if meta.include_usage {
        push_event(&mut events, usage_event(meta));
    }
    events.push_str("data: [DONE]\n\n");
    events
}

fn push_tool_call_events(
    events: &mut String,
    id: &str,
    model: &str,
    calls: &[ApiToolCall],
    reasoning: Option<&str>,
) {
    let argument_chunks = calls
        .iter()
        .map(|call| split_arguments(&call.arguments))
        .collect::<Vec<_>>();
    let mut event = json!({
        "id": id,
        "object": "chat.completion.chunk",
        "model": model,
        "choices": [{
            "index": 0,
            "delta": {
                "tool_calls": calls.iter().enumerate().map(|(index, call)| json!({
                    "index": index,
                    "id": call.id,
                    "type": "function",
                    "function": {
                        "name": call.name,
                        "arguments": argument_chunks[index].first().cloned().unwrap_or_default()
                    }
                })).collect::<Vec<_>>()
            },
            "finish_reason": null
        }]
    });
    if let Some(reasoning) = reasoning {
        event["choices"][0]["delta"]["reasoning_content"] = reasoning.into();
    }
    push_event(events, event);
    let chunk_count = argument_chunks
        .iter()
        .map(Vec::len)
        .max()
        .unwrap_or_default();
    for chunk_index in 1..chunk_count {
        let finish_reason = (chunk_index + 1 == chunk_count).then_some("tool_calls");
        push_event(
            events,
            json!({
                "id": id,
                "object": "chat.completion.chunk",
                "model": model,
                "choices": [{
                    "index": 0,
                    "delta": {
                        "tool_calls": argument_chunks.iter().enumerate().map(|(index, chunks)| json!({
                            "index": index,
                            "function": {
                                "arguments": chunks.get(chunk_index).cloned().unwrap_or_default()
                            }
                        })).collect::<Vec<_>>()
                    },
                    "finish_reason": finish_reason
                }],
            }),
        );
    }
}

fn mixed_events(
    meta: &ResponseMeta,
    reasoning: &str,
    text: Option<&str>,
    call: Option<&ApiToolCall>,
) -> String {
    let ResponseMeta { id, model, .. } = meta;
    let mut events = String::new();
    for chunk in split_reply(reasoning) {
        push_event(
            &mut events,
            json!({
                "id": id,
                "object": "chat.completion.chunk",
                "model": model,
                "choices": [{
                    "index": 0,
                    "delta": { "reasoning_content": chunk },
                    "finish_reason": null
                }]
            }),
        );
    }
    if let Some(text) = text {
        for chunk in split_reply(text) {
            push_event(
                &mut events,
                json!({
                    "id": id,
                    "object": "chat.completion.chunk",
                    "model": model,
                    "choices": [{
                        "index": 0,
                        "delta": { "content": chunk },
                        "finish_reason": null
                    }]
                }),
            );
        }
    }
    if let Some(call) = call {
        push_tool_call_events(
            &mut events,
            id,
            model,
            std::slice::from_ref(call),
            Some(reasoning),
        );
    } else {
        push_event(
            &mut events,
            json!({
                "id": id,
                "object": "chat.completion.chunk",
                "model": model,
                "choices": [{
                    "index": 0,
                    "delta": {},
                    "finish_reason": "stop"
                }]
            }),
        );
    }
    if meta.include_usage {
        push_event(&mut events, usage_event(meta));
    }
    events.push_str("data: [DONE]\n\n");
    events
}

fn usage_event(meta: &ResponseMeta) -> Value {
    let ResponseMeta {
        id,
        model,
        input_tokens,
        output_tokens,
        cache_read_tokens,
        cache_write_tokens,
        ..
    } = meta;
    json!({
        "id": id,
        "object": "chat.completion.chunk",
        "model": model,
        "choices": [],
        "usage": {
            "prompt_tokens": input_tokens,
            "completion_tokens": output_tokens,
            "total_tokens": input_tokens + output_tokens,
            "cache_read_input_tokens": cache_read_tokens,
            "cache_creation_input_tokens": cache_write_tokens
        }
    })
}

fn api_error(message: impl Into<String>) -> Value {
    json!({
        "error": {
            "message": message.into(),
            "type": "server_error"
        }
    })
}

fn push_event(events: &mut String, value: Value) {
    events.push_str("data: ");
    events.push_str(&value.to_string());
    events.push_str("\n\n");
}

fn split_reply(text: &str) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut chunk = String::new();
    let mut spaces = 0;
    for character in text.chars() {
        chunk.push(character);
        if character == ' ' {
            spaces += 1;
            if spaces == 2 {
                chunks.push(std::mem::take(&mut chunk));
                spaces = 0;
            }
        }
    }
    if !chunk.is_empty() {
        chunks.push(chunk);
    }
    if chunks.is_empty() {
        chunks.push(String::new());
    }
    chunks
}

fn split_arguments(arguments: &str) -> Vec<String> {
    let characters = arguments.chars().collect::<Vec<_>>();
    let midpoint = characters.len().div_ceil(2);
    vec![
        characters[..midpoint].iter().collect(),
        characters[midpoint..].iter().collect(),
    ]
}

fn sse_response(body: String) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(body, "text/event-stream")
}
