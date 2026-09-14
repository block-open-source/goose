//! In-process uniffi bindings for the GDK.
//!
//! This is the API surface exposed to Python and Kotlin. It focuses on native
//! Goose providers and mirrors the provider message/tool/streaming model closely
//! enough for Kotlin agent frameworks to avoid JSON-only shims for common paths.

use std::{collections::HashMap, future::Future, sync::Arc, sync::OnceLock, time::Duration};

use base64::Engine as _;
use futures::StreamExt;
use goose_providers::{
    anthropic::AnthropicProviderBuilder,
    api_client::{ApiClient, AuthMethod},
    base::{MessageStream, Provider as GooseProvider},
    conversation::{
        message::{Message, MessageContent as GooseMessageContent},
        token_usage::ProviderUsage,
    },
    databricks::DatabricksProvider as GooseDatabricksProvider,
    databricks_auth::DatabricksAuth,
    databricks_v2::DatabricksV2Provider as GooseDatabricksV2Provider,
    declarative::{DeclarativeProviderConfig, EnvKeyResolver},
    documents::{document_media_type_is_supported, SUPPORTED_DOCUMENT_MEDIA_TYPES},
    model::ModelConfig,
    openai::OpenAiProviderBuilder,
    utils::sanitize_unicode_tags,
};
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ContentBlock, ErrorCode, ErrorData, Role, Tool,
};
use serde_json::Value;

use crate::observability::{RequestDescriptor, RequestObserver, RequestOperation};

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum GooseError {
    #[error("Rate limit exceeded{retry_after_suffix}")]
    RateLimited {
        retry_after_ms: Option<u64>,
        retry_after_suffix: String,
    },
    #[error("Output token limit exceeded: {details}")]
    OutputTokenLimitExceeded { details: String },
    #[error("Context length exceeded: {details}")]
    ContextLengthExceeded { details: String },
    #[error("Authentication error: {details}")]
    Authentication { details: String },
    #[error("Timeout: {details}")]
    Timeout { details: String },
    #[error("Provider unavailable: {details}")]
    ProviderUnavailable { details: String },
    #[error("{details}")]
    Generic { details: String },
}

impl GooseError {
    fn generic(error: impl ToString) -> Self {
        Self::Generic {
            details: error.to_string(),
        }
    }
}

impl From<anyhow::Error> for GooseError {
    fn from(error: anyhow::Error) -> Self {
        Self::generic(error)
    }
}

impl From<goose_providers::errors::ProviderError> for GooseError {
    fn from(error: goose_providers::errors::ProviderError) -> Self {
        match error {
            goose_providers::errors::ProviderError::Authentication(message) => {
                Self::Authentication { details: message }
            }
            goose_providers::errors::ProviderError::ContextLengthExceeded(message) => {
                Self::ContextLengthExceeded { details: message }
            }
            goose_providers::errors::ProviderError::RateLimitExceeded { retry_delay, .. } => {
                let retry_after_ms = retry_delay.map(|delay| delay.as_millis() as u64);
                let retry_after_suffix = retry_after_ms
                    .map(|ms| format!("; retry after {ms}ms"))
                    .unwrap_or_default();
                Self::RateLimited {
                    retry_after_ms,
                    retry_after_suffix,
                }
            }
            goose_providers::errors::ProviderError::ServerError(message)
            | goose_providers::errors::ProviderError::EndpointNotFound(message)
            | goose_providers::errors::ProviderError::CreditsExhausted {
                details: message, ..
            } => Self::ProviderUnavailable { details: message },
            goose_providers::errors::ProviderError::NetworkError(message)
                if is_timeout(&message) =>
            {
                Self::Timeout { details: message }
            }
            goose_providers::errors::ProviderError::RequestFailed(message)
                if is_timeout(&message) =>
            {
                Self::Timeout { details: message }
            }
            goose_providers::errors::ProviderError::ExecutionError(message)
                if is_output_token_limit(&message) =>
            {
                Self::OutputTokenLimitExceeded { details: message }
            }
            other => Self::generic(other),
        }
    }
}

impl From<serde_json::Error> for GooseError {
    fn from(error: serde_json::Error) -> Self {
        Self::generic(error)
    }
}

fn is_timeout(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("timed out") || message.contains("timeout")
}

fn is_output_token_limit(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("output token")
        || message.contains("max_tokens")
        || message.contains("max tokens")
}

/// Receives provider request logs as JSONL records.
///
/// `start` returns an identifier that is passed to `write` for every record in
/// that request, allowing callers to keep concurrent request logs separate.
#[uniffi::export(callback_interface)]
pub trait RequestLogger: Send + Sync {
    fn start(&self) -> Result<u64, GooseError>;
    fn write(&self, request_id: u64, record: String) -> Result<(), GooseError>;
}

struct RequestLoggerAdapter {
    logger: Arc<dyn RequestLogger>,
}

impl goose_providers::request_log::RequestLogger for RequestLoggerAdapter {
    fn start(
        &self,
    ) -> Result<
        Box<dyn goose_providers::request_log::RequestLogHandle>,
        Box<dyn std::error::Error + Send + Sync>,
    > {
        Ok(Box::new(RequestLogHandleAdapter {
            request_id: self.logger.start()?,
            logger: Arc::clone(&self.logger),
        }))
    }
}

struct RequestLogHandleAdapter {
    request_id: u64,
    logger: Arc<dyn RequestLogger>,
}

impl goose_providers::request_log::RequestLogHandle for RequestLogHandleAdapter {
    fn write(&mut self, record: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.logger.write(self.request_id, record.to_string())?;
        Ok(())
    }
}

/// Installs the process-wide provider request logger.
///
/// A logger can only be installed once for the lifetime of the process.
#[uniffi::export]
pub fn install_request_logger(logger: Box<dyn RequestLogger>) -> Result<(), GooseError> {
    goose_providers::request_log::install_logger(RequestLoggerAdapter {
        logger: Arc::from(logger),
    })
    .map_err(|error| GooseError::generic(error.to_string()))
}

/// A text message passed to a provider.
#[derive(Debug, Clone, uniffi::Record)]
pub struct ProviderMessage {
    pub role: MessageRole,
    pub content: Vec<MessageContent>,
}

#[derive(Debug, Clone, uniffi::Enum)]
pub enum MessageRole {
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, uniffi::Enum)]
pub enum MessageContent {
    Text {
        text: String,
    },
    Image {
        mime_type: String,
        data: Vec<u8>,
    },
    Document {
        mime_type: String,
        data: Vec<u8>,
        name: Option<String>,
    },
    ToolRequest {
        id: String,
        name: String,
        arguments_json: String,
        #[uniffi(default = None)]
        provider_metadata_json: Option<String>,
        #[uniffi(default = None)]
        tool_error_json: Option<String>,
    },
    ToolResult {
        id: String,
        success: bool,
        content_json: String,
    },
    Thinking {
        thinking: String,
        signature: String,
    },
    RedactedThinking {
        data: String,
    },
}

impl ProviderMessage {
    fn to_goose_message(&self) -> Result<Option<Message>, GooseError> {
        let role = match self.role {
            MessageRole::User | MessageRole::Tool => Role::User,
            MessageRole::Assistant => Role::Assistant,
        };
        let mut message = Message::new(role, chrono_now(), Vec::new());
        for content in &self.content {
            message = message.with_content(content.to_goose_content()?);
        }
        Ok(Some(message))
    }
}

impl MessageContent {
    fn to_goose_content(&self) -> Result<GooseMessageContent, GooseError> {
        match self {
            MessageContent::Text { text } => {
                Ok(GooseMessageContent::text(sanitize_unicode_tags(text)))
            }
            MessageContent::Image { mime_type, data } => Ok(GooseMessageContent::image(
                base64::engine::general_purpose::STANDARD.encode(data),
                mime_type.clone(),
            )),
            MessageContent::Document {
                mime_type,
                data,
                name,
            } => {
                if !document_media_type_is_supported(mime_type) {
                    return Err(GooseError::generic(format!(
                        "unsupported document media type {mime_type}: supported types are {}",
                        SUPPORTED_DOCUMENT_MEDIA_TYPES.join(", ")
                    )));
                }
                Ok(GooseMessageContent::document(
                    base64::engine::general_purpose::STANDARD.encode(data),
                    mime_type.clone(),
                    name.clone(),
                ))
            }
            MessageContent::ToolRequest {
                id,
                name,
                arguments_json,
                provider_metadata_json,
                tool_error_json,
            } => {
                let metadata = provider_metadata_json
                    .as_deref()
                    .map(parse_json_object)
                    .transpose()?;
                let tool_call = match tool_error_json {
                    Some(error_json) => Err(serde_json::from_str(error_json)?),
                    None => {
                        let arguments = parse_json_object(arguments_json)?;
                        Ok(CallToolRequestParams::new(name.clone()).with_arguments(arguments))
                    }
                };
                Ok(GooseMessageContent::tool_request_with_metadata(
                    id.clone(),
                    tool_call,
                    metadata.as_ref(),
                ))
            }
            MessageContent::ToolResult {
                id,
                success,
                content_json,
            } => {
                let value: Value = serde_json::from_str(content_json)?;
                let tool_result = if *success {
                    Ok(call_tool_result(value, false))
                } else {
                    Err(ErrorData::new(
                        ErrorCode::INTERNAL_ERROR,
                        value.to_string(),
                        None,
                    ))
                };
                Ok(GooseMessageContent::tool_response(id.clone(), tool_result))
            }
            MessageContent::Thinking {
                thinking,
                signature,
            } => Ok(GooseMessageContent::thinking(
                thinking.clone(),
                signature.clone(),
            )),
            MessageContent::RedactedThinking { data } => {
                Ok(GooseMessageContent::redacted_thinking(data.clone()))
            }
        }
    }

    /// Maps provider output back onto the binding surface so callers can replay
    /// an assistant turn without reparsing `message_json`. Thinking signatures
    /// and redacted payloads are carried through verbatim: providers reject
    /// replayed thinking blocks whose signature was dropped or altered.
    fn from_goose_content(content: &GooseMessageContent) -> Option<Self> {
        match content {
            GooseMessageContent::Text(text) => Some(MessageContent::Text {
                text: text.text.clone(),
            }),
            GooseMessageContent::Image(image) => Some(MessageContent::Image {
                mime_type: image.mime_type.clone(),
                data: base64::engine::general_purpose::STANDARD
                    .decode(&image.data)
                    .ok()?,
            }),
            GooseMessageContent::ToolRequest(request) => {
                let provider_metadata_json = request
                    .metadata
                    .as_ref()
                    .and_then(|metadata| serde_json::to_string(metadata).ok());
                match &request.tool_call {
                    Ok(tool_call) => Some(MessageContent::ToolRequest {
                        id: request.id.clone(),
                        name: tool_call.name.to_string(),
                        arguments_json: serde_json::to_string(
                            &tool_call.arguments.clone().unwrap_or_default(),
                        )
                        .ok()?,
                        provider_metadata_json,
                        tool_error_json: None,
                    }),
                    Err(error) => Some(MessageContent::ToolRequest {
                        id: request.id.clone(),
                        name: String::new(),
                        arguments_json: "{}".to_string(),
                        provider_metadata_json,
                        tool_error_json: Some(serde_json::to_string(error).ok()?),
                    }),
                }
            }
            GooseMessageContent::Thinking(thinking) => Some(MessageContent::Thinking {
                thinking: thinking.thinking.clone(),
                signature: thinking.signature.clone(),
            }),
            GooseMessageContent::RedactedThinking(redacted) => {
                Some(MessageContent::RedactedThinking {
                    data: redacted.data.clone(),
                })
            }
            _ => None,
        }
    }
}

fn chrono_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

fn call_tool_result(value: Value, is_error: bool) -> CallToolResult {
    let content = match value {
        Value::Array(items) => items.into_iter().map(value_to_content).collect(),
        other => vec![value_to_content(other)],
    };

    if is_error {
        CallToolResult::error(content)
    } else {
        CallToolResult::success(content)
    }
}

fn value_to_content(value: Value) -> ContentBlock {
    match value {
        Value::String(text) => ContentBlock::text(text),
        Value::Object(object) => match object.get("type").and_then(|value| value.as_str()) {
            Some("text") => object
                .get("text")
                .and_then(|value| value.as_str().map(str::to_owned))
                .map(ContentBlock::text)
                .unwrap_or_else(|| ContentBlock::text(Value::Object(object).to_string())),
            Some("image") => {
                let mime_type = object
                    .get("mimeType")
                    .or_else(|| object.get("mime_type"))
                    .and_then(|value| value.as_str().map(str::to_owned))
                    .unwrap_or_else(|| "image/png".to_string());
                let data = object
                    .get("data")
                    .and_then(|value| value.as_str().map(str::to_owned))
                    .unwrap_or_default();
                ContentBlock::image(data, mime_type)
            }
            _ => ContentBlock::text(Value::Object(object).to_string()),
        },
        other => ContentBlock::text(other.to_string()),
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct ProviderTool {
    pub name: String,
    pub description: String,
    pub input_schema_json: String,
    #[uniffi(default = None)]
    pub annotations_json: Option<String>,
}

impl ProviderTool {
    fn to_goose_tool(&self) -> Result<Tool, GooseError> {
        let schema = parse_json_object(&self.input_schema_json)?;
        let mut tool = Tool::new(self.name.clone(), self.description.clone(), schema);
        if let Some(annotations_json) = &self.annotations_json {
            tool.annotations = Some(serde_json::from_str(annotations_json)?);
        }
        Ok(tool)
    }
}

fn parse_json_object(json: &str) -> Result<serde_json::Map<String, Value>, GooseError> {
    match serde_json::from_str(json)? {
        Value::Object(object) => Ok(object),
        _ => Err(GooseError::generic("expected a JSON object")),
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct ProviderModelConfig {
    pub model_name: String,
    #[uniffi(default = None)]
    pub context_limit: Option<i32>,
    #[uniffi(default = None)]
    pub temperature: Option<f32>,
    #[uniffi(default = None)]
    pub max_tokens: Option<i32>,
    #[uniffi(default = false)]
    pub toolshim: bool,
    #[uniffi(default = None)]
    pub toolshim_model: Option<String>,
    #[uniffi(default = None)]
    pub request_params_json: Option<String>,
    #[uniffi(default = None)]
    pub provider_params_json: Option<String>,
    #[uniffi(default = None)]
    pub reasoning: Option<bool>,
    #[uniffi(default = None)]
    pub timeout_ms: Option<u64>,
    /// Per-request HTTP headers attached to the outgoing provider call.
    /// These override any static headers configured on the provider.
    #[uniffi(default = None)]
    pub request_headers: Option<HashMap<String, String>>,
}

impl ProviderModelConfig {
    fn to_goose_model_config(&self) -> Result<ModelConfig, GooseError> {
        let mut config = ModelConfig::new(&self.model_name)
            .with_temperature(self.temperature)
            .with_max_tokens(self.max_tokens)
            .with_toolshim(self.toolshim)
            .with_toolshim_model(self.toolshim_model.clone());

        let mut request_params = serde_json::Map::new();
        merge_params(&mut request_params, self.request_params_json.as_ref())?;
        merge_params(&mut request_params, self.provider_params_json.as_ref())?;
        if !request_params.is_empty() {
            config = config.with_merged_request_params(request_params.into_iter().collect());
        }

        config = config.with_request_headers(self.request_headers.clone());
        config.reasoning = self.reasoning;
        Ok(config)
    }
}

fn merge_params(
    target: &mut serde_json::Map<String, Value>,
    params_json: Option<&String>,
) -> Result<(), GooseError> {
    if let Some(params_json) = params_json {
        for (key, value) in parse_json_object(params_json)? {
            target.insert(key, value);
        }
    }
    Ok(())
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct Usage {
    pub input_tokens: Option<i32>,
    pub output_tokens: Option<i32>,
    pub total_tokens: Option<i32>,
    pub cache_read_input_tokens: Option<i32>,
    pub cache_creation_input_tokens: Option<i32>,
    pub reasoning_tokens: Option<i32>,
    pub model: String,
    pub provider_metadata_json: Option<String>,
    /// Provider-specific response fields as a JSON object, present only when the
    /// provider reported fields with no canonical `Usage` equivalent.
    pub additional_data_json: Option<String>,
}

impl Usage {
    fn from_provider_usage(usage: &ProviderUsage) -> Result<Self, GooseError> {
        Ok(Self {
            input_tokens: usage.usage.input_tokens,
            output_tokens: usage.usage.output_tokens,
            total_tokens: usage.usage.total_tokens,
            cache_read_input_tokens: usage.usage.cache_read_input_tokens,
            cache_creation_input_tokens: usage.usage.cache_write_input_tokens,
            reasoning_tokens: None,
            model: usage.model.clone(),
            provider_metadata_json: Some(serde_json::to_string(usage)?),
            additional_data_json: usage
                .additional_data
                .as_ref()
                .map(serde_json::to_string)
                .transpose()?,
        })
    }
}

#[derive(Debug, Clone, uniffi::Enum)]
pub enum StreamChunk {
    TextChunk {
        text: String,
    },
    ToolChunk {
        id: String,
        name: String,
        arguments_json: String,
        index: Option<i32>,
        #[uniffi(default = None)]
        provider_metadata_json: Option<String>,
    },
    ThinkingChunk {
        thinking: String,
        signature: String,
    },
    RedactedThinkingChunk {
        data: String,
    },
    EndChunk {
        usage: Option<Usage>,
    },
    ErrorChunk {
        error: GooseStreamError,
    },
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct GooseStreamError {
    pub kind: GooseStreamErrorKind,
    pub message: String,
    pub retry_after_ms: Option<u64>,
}

#[derive(Debug, Clone, uniffi::Enum)]
pub enum GooseStreamErrorKind {
    RateLimited,
    OutputTokenLimitExceeded,
    ContextLengthExceeded,
    Authentication,
    Timeout,
    ProviderUnavailable,
    Generic,
}

impl From<&GooseError> for GooseStreamError {
    fn from(error: &GooseError) -> Self {
        match error {
            GooseError::RateLimited {
                retry_after_ms,
                retry_after_suffix,
            } => Self {
                kind: GooseStreamErrorKind::RateLimited,
                message: format!("Rate limit exceeded{retry_after_suffix}"),
                retry_after_ms: *retry_after_ms,
            },
            GooseError::OutputTokenLimitExceeded { details } => Self {
                kind: GooseStreamErrorKind::OutputTokenLimitExceeded,
                message: details.clone(),
                retry_after_ms: None,
            },
            GooseError::ContextLengthExceeded { details } => Self {
                kind: GooseStreamErrorKind::ContextLengthExceeded,
                message: details.clone(),
                retry_after_ms: None,
            },
            GooseError::Authentication { details } => Self {
                kind: GooseStreamErrorKind::Authentication,
                message: details.clone(),
                retry_after_ms: None,
            },
            GooseError::Timeout { details } => Self {
                kind: GooseStreamErrorKind::Timeout,
                message: details.clone(),
                retry_after_ms: None,
            },
            GooseError::ProviderUnavailable { details } => Self {
                kind: GooseStreamErrorKind::ProviderUnavailable,
                message: details.clone(),
                retry_after_ms: None,
            },
            GooseError::Generic { details } => Self {
                kind: GooseStreamErrorKind::Generic,
                message: details.clone(),
                retry_after_ms: None,
            },
        }
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct ProviderCompletion {
    pub message_json: String,
    /// The assistant turn as binding types, ready to append to history and
    /// replay on the next request without reparsing `message_json`.
    pub content: Vec<MessageContent>,
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, uniffi::Enum)]
pub enum Feature {
    Tools,
    Streaming,
    Images,
    Documents,
    JsonSchema,
    Reasoning,
}

static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

fn runtime() -> Result<&'static tokio::runtime::Runtime, GooseError> {
    if let Some(runtime) = RUNTIME.get() {
        return Ok(runtime);
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(GooseError::generic)?;

    if let Err(unused) = RUNTIME.set(runtime) {
        // Lost the init race; dropping a runtime inside an async context
        // panics, so shut it down without blocking.
        unused.shutdown_background();
    }
    Ok(RUNTIME.get().expect("runtime was initialized"))
}

struct AbortOnDrop<T> {
    handle: tokio::task::JoinHandle<T>,
}

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

async fn run_on_runtime<T>(
    future: impl Future<Output = T> + Send + 'static,
) -> Result<T, GooseError>
where
    T: Send + 'static,
{
    let mut task = AbortOnDrop {
        handle: runtime()?.spawn(future),
    };
    (&mut task.handle).await.map_err(|error| {
        if error.is_cancelled() {
            GooseError::Timeout {
                details: "runtime task was cancelled".to_string(),
            }
        } else {
            GooseError::generic(error)
        }
    })
}

struct ProviderHandle {
    provider: Arc<dyn GooseProvider>,
}

impl ProviderHandle {
    fn new(provider: Box<dyn GooseProvider>) -> Self {
        Self {
            provider: Arc::from(provider),
        }
    }

    fn name(&self) -> String {
        self.provider.get_name().to_string()
    }

    async fn context_limit(&self, model: ProviderModelConfig) -> Result<usize, GooseError> {
        let normalized_model = ModelConfig::new(&model.model_name);
        let override_limit = model
            .context_limit
            .and_then(|limit| (limit > 0).then_some(limit as usize));
        let provider = Arc::clone(&self.provider);
        run_on_runtime(async move {
            provider
                .get_context_limit(&normalized_model.model_name, override_limit)
                .await
        })
        .await
    }

    async fn stream(
        &self,
        model: ProviderModelConfig,
        system: String,
        messages: Vec<ProviderMessage>,
        tools: Vec<ProviderTool>,
    ) -> Result<Arc<ProviderStream>, GooseError> {
        let timeout_ms = model.timeout_ms;
        let model = model.to_goose_model_config()?;
        let messages = convert_messages(messages)?;
        let tools = convert_tools(tools)?;
        let observer = Arc::new(RequestObserver::start(RequestDescriptor {
            provider: self.provider.get_name(),
            model: &model.model_name,
            operation: RequestOperation::Stream,
            system: &system,
            messages: &messages,
            tools: &tools,
        }));
        let provider = Arc::clone(&self.provider);
        let stream = match run_provider_future(timeout_ms, async move {
            provider.stream(&model, &system, &messages, &tools).await
        })
        .await
        {
            Ok(Ok(stream)) => stream,
            Ok(Err(error)) => return Err(observer.fail(GooseError::from(error))),
            Err(error) => return Err(observer.fail(error)),
        };
        observer.response_started();

        Ok(Arc::new(ProviderStream {
            state: Arc::new(tokio::sync::Mutex::new(ProviderStreamState {
                stream,
                pending: Vec::new(),
                final_usage: None,
                ended: false,
            })),
            timeout_ms,
            observer,
        }))
    }

    async fn complete(
        &self,
        model: ProviderModelConfig,
        system: String,
        messages: Vec<ProviderMessage>,
        tools: Vec<ProviderTool>,
    ) -> Result<ProviderCompletion, GooseError> {
        let timeout_ms = model.timeout_ms;
        let model = model.to_goose_model_config()?;
        let messages = convert_messages(messages)?;
        let tools = convert_tools(tools)?;
        let observer = RequestObserver::start(RequestDescriptor {
            provider: self.provider.get_name(),
            model: &model.model_name,
            operation: RequestOperation::Complete,
            system: &system,
            messages: &messages,
            tools: &tools,
        });
        let provider = Arc::clone(&self.provider);
        let (message, usage) = match run_provider_future(timeout_ms, async move {
            provider.complete(&model, &system, &messages, &tools).await
        })
        .await
        {
            Ok(Ok(completion)) => completion,
            Ok(Err(error)) => return Err(observer.fail(GooseError::from(error))),
            Err(error) => return Err(observer.fail(error)),
        };
        observer.response_started();

        let completion = ProviderCompletion {
            message_json: serde_json::to_string(&message)?,
            content: message
                .content
                .iter()
                .filter_map(MessageContent::from_goose_content)
                .collect(),
            usage: Some(Usage::from_provider_usage(&usage)?),
        };
        observer.succeeded(
            completion.usage.clone(),
            observer
                .captures_payloads()
                .then(|| completion.message_json.clone()),
        );
        Ok(completion)
    }
}

async fn run_provider_future<T>(
    timeout_ms: Option<u64>,
    future: impl Future<Output = T> + Send + 'static,
) -> Result<T, GooseError>
where
    T: Send + 'static,
{
    run_on_runtime(async move {
        if let Some(timeout_ms) = timeout_ms {
            tokio::time::timeout(Duration::from_millis(timeout_ms), future)
                .await
                .map_err(|_| GooseError::Timeout {
                    details: format!("request timed out after {timeout_ms}ms"),
                })
        } else {
            Ok(future.await)
        }
    })
    .await?
}

fn convert_messages(messages: Vec<ProviderMessage>) -> Result<Vec<Message>, GooseError> {
    messages
        .iter()
        .filter_map(|message| message.to_goose_message().transpose())
        .collect()
}

fn convert_tools(tools: Vec<ProviderTool>) -> Result<Vec<Tool>, GooseError> {
    tools.iter().map(ProviderTool::to_goose_tool).collect()
}

#[derive(uniffi::Object)]
pub struct Provider {
    handle: ProviderHandle,
}

impl Provider {
    fn new(provider: Box<dyn GooseProvider>) -> Arc<Self> {
        Arc::new(Self {
            handle: ProviderHandle::new(provider),
        })
    }
}

#[uniffi::export]
impl Provider {
    pub fn name(&self) -> String {
        self.handle.name()
    }

    pub fn supported_features(&self) -> Vec<Feature> {
        let name = self.name();
        let mut features = vec![Feature::Streaming, Feature::Tools, Feature::JsonSchema];
        if matches!(
            name.as_str(),
            "openai" | "anthropic" | "databricks" | "databricks_v2" | "groq"
        ) {
            features.push(Feature::Images);
        }
        if matches!(
            name.as_str(),
            "openai" | "anthropic" | "databricks" | "databricks_v2" | "google"
        ) {
            features.push(Feature::Documents);
        }
        if matches!(
            name.as_str(),
            "openai" | "anthropic" | "databricks" | "databricks_v2"
        ) {
            features.push(Feature::Reasoning);
        }
        features
    }

    pub async fn context_limit(&self, model: ProviderModelConfig) -> Result<u64, GooseError> {
        Ok(self.handle.context_limit(model).await? as u64)
    }

    pub async fn stream(
        &self,
        model: ProviderModelConfig,
        system: String,
        messages: Vec<ProviderMessage>,
        tools: Vec<ProviderTool>,
    ) -> Result<Arc<ProviderStream>, GooseError> {
        self.handle.stream(model, system, messages, tools).await
    }

    pub async fn complete(
        &self,
        model: ProviderModelConfig,
        system: String,
        messages: Vec<ProviderMessage>,
        tools: Vec<ProviderTool>,
    ) -> Result<ProviderCompletion, GooseError> {
        self.handle.complete(model, system, messages, tools).await
    }

    /// Summarizes a conversation down to a single message so it can continue
    /// past this model's context window.
    pub async fn compact(
        &self,
        model_name: String,
        messages: Vec<CompactionMessage>,
        templates: Option<CompactionTemplates>,
    ) -> Result<CompactionSummary, GooseError> {
        let messages: Vec<Message> = messages
            .iter()
            .map(CompactionMessage::to_goose_message)
            .collect();
        let templates = templates.map(Into::into).unwrap_or_default();
        let model = goose_context_management::ProviderModel::new(
            self.handle.provider.clone(),
            ModelConfig::new(&model_name),
        );

        let summary = run_on_runtime(async move {
            goose_context_management::summarize(&model, None, &templates, &messages).await
        })
        .await?
        .map_err(GooseError::generic)?;

        Ok(CompactionSummary {
            text: summary.message.as_concat_text(),
            input_tokens: summary.usage.usage.input_tokens,
            output_tokens: summary.usage.usage.output_tokens,
            total_tokens: summary.usage.usage.total_tokens,
            cache_read_input_tokens: summary.usage.usage.cache_read_input_tokens,
            cache_creation_input_tokens: summary.usage.usage.cache_write_input_tokens,
        })
    }
}

/// A text-only message. Compaction reads conversations as text, so this is the
/// whole input shape callers need across the language boundary.
#[derive(Debug, Clone, uniffi::Record)]
pub struct CompactionMessage {
    pub role: MessageRole,
    pub text: String,
}

impl CompactionMessage {
    fn to_goose_message(&self) -> Message {
        let role = match self.role {
            MessageRole::User | MessageRole::Tool => Role::User,
            MessageRole::Assistant => Role::Assistant,
        };
        let mut message = match role {
            Role::User => Message::user(),
            Role::Assistant => Message::assistant(),
        }
        .with_text(&self.text);
        message.role = role;
        message
    }
}

/// Overrides for the summarization and summary-rendering prompts.
#[derive(Debug, Clone, uniffi::Record)]
pub struct CompactionTemplates {
    pub compaction: String,
    pub summary: String,
}

impl From<CompactionTemplates> for goose_context_management::Templates {
    fn from(value: CompactionTemplates) -> Self {
        Self {
            compaction: value.compaction,
            summary: value.summary,
        }
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct CompactionSummary {
    pub text: String,
    pub input_tokens: Option<i32>,
    pub output_tokens: Option<i32>,
    pub total_tokens: Option<i32>,
    pub cache_read_input_tokens: Option<i32>,
    pub cache_creation_input_tokens: Option<i32>,
}

#[uniffi::export]
pub fn default_compaction_templates() -> CompactionTemplates {
    let templates = goose_context_management::Templates::default();
    CompactionTemplates {
        compaction: templates.compaction,
        summary: templates.summary,
    }
}

#[uniffi::export]
pub fn declarative_provider_from_json(json: String) -> Result<Arc<Provider>, GooseError> {
    let provider = goose_providers::declarative::from_json(&json, None, EnvKeyResolver {})?;
    Ok(Provider::new(provider))
}

#[uniffi::export]
pub fn openai_default_model() -> String {
    goose_providers::openai::OPEN_AI_DEFAULT_MODEL.to_string()
}

#[uniffi::export]
pub fn openai_provider(api_key: String) -> Result<Arc<Provider>, GooseError> {
    let api_client = ApiClient::new_with_tls(
        "https://api.openai.com".to_string(),
        AuthMethod::BearerToken(api_key),
        None,
    )?;
    let provider = OpenAiProviderBuilder::new(api_client).build();
    Ok(Provider::new(Box::new(provider)))
}

#[uniffi::export]
pub fn anthropic_default_model() -> String {
    goose_providers::anthropic::ANTHROPIC_DEFAULT_MODEL.to_string()
}

#[uniffi::export]
pub fn anthropic_provider(
    api_key: String,
    base_url: Option<String>,
    beta_headers: Vec<String>,
) -> Result<Arc<Provider>, GooseError> {
    let mut api_client = ApiClient::new_with_tls(
        base_url.unwrap_or_else(|| "https://api.anthropic.com".to_string()),
        AuthMethod::ApiKey {
            header_name: "x-api-key".to_string(),
            key: api_key,
        },
        None,
    )?
    .with_header(
        "anthropic-version",
        goose_providers::anthropic::ANTHROPIC_API_VERSION,
    )?;

    if !beta_headers.is_empty() {
        api_client = api_client.with_header("anthropic-beta", &beta_headers.join(","))?;
    }

    let provider = AnthropicProviderBuilder::new(api_client)
        .format_options(goose_providers::formats::anthropic::AnthropicFormatOptions::native())
        .build();
    Ok(Provider::new(Box::new(provider)))
}

#[uniffi::export]
pub fn groq_default_model() -> String {
    groq_config()
        .models
        .first()
        .map(|model| model.name.clone())
        .unwrap_or_else(|| "moonshotai/kimi-k2-instruct-0905".to_string())
}

#[uniffi::export]
pub fn groq_provider(api_key: String) -> Result<Arc<Provider>, GooseError> {
    let json = goose_providers::groq::JSON.replace("${GROQ_API_KEY}", &api_key);
    let provider = goose_providers::declarative::from_json(
        &json,
        None,
        StaticKeyResolver {
            key_name: "GROQ_API_KEY".to_string(),
            key: api_key,
        },
    )?;
    Ok(Provider::new(provider))
}

fn groq_config() -> DeclarativeProviderConfig {
    goose_providers::declarative::deserialize_provider_config(goose_providers::groq::JSON)
        .expect("bundled groq provider config is valid")
}

struct StaticKeyResolver {
    key_name: String,
    key: String,
}

impl goose_providers::declarative::KeyResolver for StaticKeyResolver {
    type Error = std::env::VarError;

    fn resolve_key(&self, key: &str) -> std::result::Result<String, Self::Error> {
        if key == self.key_name {
            Ok(self.key.clone())
        } else {
            std::env::var(key)
        }
    }
}

#[uniffi::export]
pub fn databricks_default_model() -> String {
    goose_providers::databricks::DATABRICKS_DEFAULT_MODEL.to_string()
}

#[uniffi::export]
pub fn databricks_provider(host: String, token: String) -> Result<Arc<Provider>, GooseError> {
    let retry_config = GooseDatabricksProvider::load_retry_config(|key| std::env::var(key).ok());
    let provider = GooseDatabricksProvider::new(
        host,
        DatabricksAuth::token(token),
        retry_config,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    )?;

    Ok(Provider::new(Box::new(provider)))
}

#[uniffi::export]
pub fn databricks_v2_default_model() -> String {
    goose_providers::databricks_v2::DATABRICKS_V2_DEFAULT_MODEL.to_string()
}

#[uniffi::export(default(gateway_path = None))]
pub fn databricks_v2_provider(
    host: String,
    token: String,
    gateway_path: Option<String>,
) -> Result<Arc<Provider>, GooseError> {
    let retry_config = GooseDatabricksV2Provider::load_retry_config(|key| std::env::var(key).ok());
    let provider = GooseDatabricksV2Provider::new(
        host,
        DatabricksAuth::token(token),
        retry_config,
        None,
        None,
        None,
        None,
        None,
    )?;
    let provider = match gateway_path {
        Some(path) => provider.with_gateway_path(&path)?,
        None => provider,
    };

    Ok(Provider::new(Box::new(provider)))
}

#[derive(uniffi::Object)]
pub struct ProviderStream {
    state: Arc<tokio::sync::Mutex<ProviderStreamState>>,
    timeout_ms: Option<u64>,
    observer: Arc<RequestObserver>,
}

struct ProviderStreamState {
    stream: MessageStream,
    pending: Vec<StreamChunk>,
    final_usage: Option<Usage>,
    ended: bool,
}

#[uniffi::export]
impl ProviderStream {
    pub async fn next_chunk(&self) -> Result<Option<StreamChunk>, GooseError> {
        let state = Arc::clone(&self.state);
        let timeout_ms = self.timeout_ms;
        let observer = Arc::clone(&self.observer);
        run_on_runtime(async move {
            let mut state = state.lock().await;
            loop {
                if let Some(chunk) = state.pending.pop() {
                    return Ok(Some(chunk));
                }

                if state.ended {
                    return Ok(None);
                }

                let next = if let Some(timeout_ms) = timeout_ms {
                    match tokio::time::timeout(
                        Duration::from_millis(timeout_ms),
                        state.stream.next(),
                    )
                    .await
                    {
                        Ok(next) => next,
                        Err(_) => {
                            return Err(observer.fail(GooseError::Timeout {
                                details: format!("request timed out after {timeout_ms}ms"),
                            }))
                        }
                    }
                } else {
                    state.stream.next().await
                };

                match next {
                    Some(Ok((message, usage))) => {
                        if let Some(usage) = usage {
                            match Usage::from_provider_usage(&usage) {
                                Ok(usage) => state.final_usage = Some(usage),
                                Err(error) => {
                                    state.ended = true;
                                    return Err(observer.fail(error));
                                }
                            }
                        }
                        let Some(message) = message else {
                            continue;
                        };
                        let mut chunks = message_to_chunks(message);
                        if chunks.is_empty() {
                            continue;
                        }
                        chunks.reverse();
                        let first = chunks.pop();
                        state.pending = chunks;
                        return Ok(first);
                    }
                    Some(Err(error)) => {
                        state.ended = true;
                        let error = GooseStreamError::from(&GooseError::from(error));
                        observer.fail_stream(error.clone());
                        return Ok(Some(StreamChunk::ErrorChunk { error }));
                    }
                    None => {
                        state.ended = true;
                        let usage = state.final_usage.clone();
                        observer.succeeded(usage.clone(), None);
                        return Ok(Some(StreamChunk::EndChunk { usage }));
                    }
                }
            }
        })
        .await?
    }
}

fn message_to_chunks(message: Message) -> Vec<StreamChunk> {
    message
        .content
        .into_iter()
        .filter_map(|content| match content {
            GooseMessageContent::Text(text) if !text.text.is_empty() => {
                Some(StreamChunk::TextChunk {
                    text: text.text.clone(),
                })
            }
            GooseMessageContent::ToolRequest(request) => {
                let index = request.provider_index();
                let provider_metadata_json = request
                    .metadata
                    .as_ref()
                    .and_then(|metadata| serde_json::to_string(metadata).ok());
                match request.tool_call {
                    Ok(tool_call) => Some(StreamChunk::ToolChunk {
                        index,
                        id: request.id,
                        name: tool_call.name.to_string(),
                        arguments_json: serde_json::to_string(
                            &tool_call.arguments.unwrap_or_default(),
                        )
                        .unwrap_or_else(|_| "{}".to_string()),
                        provider_metadata_json,
                    }),
                    Err(error) => Some(StreamChunk::ErrorChunk {
                        error: GooseStreamError {
                            kind: GooseStreamErrorKind::Generic,
                            message: error.to_string(),
                            retry_after_ms: None,
                        },
                    }),
                }
            }
            GooseMessageContent::Thinking(thinking) => Some(StreamChunk::ThinkingChunk {
                thinking: thinking.thinking,
                signature: thinking.signature,
            }),
            GooseMessageContent::RedactedThinking(redacted) => {
                Some(StreamChunk::RedactedThinkingChunk {
                    data: redacted.data,
                })
            }
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_model_config() -> ProviderModelConfig {
        ProviderModelConfig {
            model_name: "test".to_string(),
            context_limit: None,
            temperature: None,
            max_tokens: None,
            toolshim: false,
            toolshim_model: None,
            request_params_json: None,
            provider_params_json: None,
            reasoning: None,
            timeout_ms: None,
            request_headers: None,
        }
    }

    #[test]
    fn context_limit_override_filters_nonpositive_values() {
        let none = base_model_config();
        assert_eq!(
            none.context_limit
                .and_then(|limit| (limit > 0).then_some(limit as usize)),
            None
        );

        let negative = ProviderModelConfig {
            context_limit: Some(-1),
            ..base_model_config()
        };
        assert_eq!(
            negative
                .context_limit
                .and_then(|limit| (limit > 0).then_some(limit as usize)),
            None
        );

        let positive = ProviderModelConfig {
            context_limit: Some(64_000),
            ..base_model_config()
        };
        assert_eq!(
            positive
                .context_limit
                .and_then(|limit| (limit > 0).then_some(limit as usize)),
            Some(64_000)
        );
    }

    #[test]
    fn model_config_normalizes_effort_suffix() {
        assert_eq!(ModelConfig::new("gpt-5.4-xhigh").model_name, "gpt-5.4");
    }

    #[test]
    fn model_config_rejects_invalid_request_params_json() {
        let config = ProviderModelConfig {
            request_params_json: Some("not json".to_string()),
            ..base_model_config()
        };

        assert!(config.to_goose_model_config().is_err());
    }

    #[test]
    fn provider_message_converts_user_text() {
        let message = ProviderMessage {
            role: MessageRole::User,
            content: vec![MessageContent::Text {
                text: "what is the capital of France?".to_string(),
            }],
        }
        .to_goose_message()
        .unwrap()
        .unwrap();

        assert_eq!(message.as_concat_text(), "what is the capital of France?");
    }

    #[test]
    fn provider_message_converts_document_to_base64_content() {
        let message = ProviderMessage {
            role: MessageRole::User,
            content: vec![MessageContent::Document {
                mime_type: "application/pdf".to_string(),
                data: b"pdf-bytes".to_vec(),
                name: Some("q3-report.pdf".to_string()),
            }],
        }
        .to_goose_message()
        .unwrap()
        .unwrap();

        let GooseMessageContent::Document(document) = &message.content[0] else {
            panic!("expected document content");
        };
        assert_eq!(document.mime_type, "application/pdf");
        assert_eq!(document.name.as_deref(), Some("q3-report.pdf"));
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(&document.data)
                .unwrap(),
            b"pdf-bytes"
        );
    }

    #[test]
    fn provider_message_converts_document_serializes_for_anthropic() {
        let message = ProviderMessage {
            role: MessageRole::User,
            content: vec![MessageContent::Document {
                mime_type: "application/pdf".to_string(),
                data: b"pdf-bytes".to_vec(),
                name: Some("q3-report.pdf".to_string()),
            }],
        }
        .to_goose_message()
        .unwrap()
        .unwrap();

        let spec = goose_providers::formats::anthropic::format_messages(&[message]);
        let block = &spec[0]["content"][0];

        assert_eq!(block["type"], "document");
        assert_eq!(block["title"], "q3-report.pdf");
        assert_eq!(block["source"]["type"], "base64");
        assert_eq!(block["source"]["media_type"], "application/pdf");
        assert_eq!(block["source"]["data"], "cGRmLWJ5dGVz");
    }

    #[test]
    fn provider_message_rejects_unsupported_document_media_type() {
        let error = MessageContent::Document {
            mime_type: "text/csv".to_string(),
            data: b"rows".to_vec(),
            name: Some("rows.csv".to_string()),
        }
        .to_goose_content()
        .unwrap_err();

        assert!(error.to_string().contains("text/csv"), "{error}");
        assert!(error.to_string().contains("application/pdf"), "{error}");
    }

    #[test]
    fn tool_config_converts_to_rmcp_tool() {
        let tool = ProviderTool {
            name: "lookup".to_string(),
            description: "Lookup a value".to_string(),
            input_schema_json: r#"{"type":"object","properties":{"key":{"type":"string"}}}"#
                .to_string(),
            annotations_json: None,
        }
        .to_goose_tool()
        .unwrap();

        assert_eq!(tool.name.as_ref(), "lookup");
        assert_eq!(tool.input_schema["type"], "object");
    }

    #[test]
    fn usage_exposes_provider_additional_data() {
        let mut provider_usage = ProviderUsage::new(
            "claude-sonnet-4-5".to_string(),
            goose_providers::conversation::token_usage::Usage::new(Some(10), Some(5), None),
        );
        provider_usage.additional_data = Some(
            serde_json::json!({ "service_tier": "fast" })
                .as_object()
                .unwrap()
                .clone(),
        );

        let additional_data_json = Usage::from_provider_usage(&provider_usage)
            .unwrap()
            .additional_data_json
            .expect("additional data should be exposed");

        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&additional_data_json).unwrap(),
            serde_json::json!({ "service_tier": "fast" })
        );
    }

    #[test]
    fn usage_omits_provider_additional_data_when_absent() {
        let provider_usage = ProviderUsage::new(
            "claude-sonnet-4-5".to_string(),
            goose_providers::conversation::token_usage::Usage::new(Some(10), Some(5), None),
        );

        assert!(Usage::from_provider_usage(&provider_usage)
            .unwrap()
            .additional_data_json
            .is_none());
    }

    #[test]
    fn tool_result_content_converts() {
        let content = MessageContent::ToolResult {
            id: "call_1".to_string(),
            success: true,
            content_json: r#"{"type":"text","text":"done"}"#.to_string(),
        }
        .to_goose_content()
        .unwrap();

        let GooseMessageContent::ToolResponse(response) = content else {
            panic!("expected tool response");
        };
        let result = response.tool_result.unwrap();
        assert_eq!(result.is_error, Some(false));
        assert_eq!(result.content[0].as_text().unwrap().text, "done");
    }

    fn provider_usage_with_cache(
        cache_read: Option<i32>,
        cache_write: Option<i32>,
    ) -> ProviderUsage {
        use goose_providers::conversation::token_usage::Usage as GooseUsage;

        ProviderUsage::new(
            "test-model".to_string(),
            GooseUsage::new(Some(100), Some(20), Some(120))
                .with_cache_tokens(cache_read, cache_write),
        )
    }

    #[test]
    fn usage_exposes_reported_cache_tokens() {
        let usage = Usage::from_provider_usage(&provider_usage_with_cache(Some(80), Some(5)))
            .expect("conversion");

        assert_eq!(usage.cache_read_input_tokens, Some(80));
        assert_eq!(usage.cache_creation_input_tokens, Some(5));
    }

    #[test]
    fn usage_distinguishes_reported_zero_cache_tokens_from_absence() {
        let zero =
            Usage::from_provider_usage(&provider_usage_with_cache(Some(0), Some(0))).unwrap();
        assert_eq!(zero.cache_read_input_tokens, Some(0));
        assert_eq!(zero.cache_creation_input_tokens, Some(0));

        let absent = Usage::from_provider_usage(&provider_usage_with_cache(None, None)).unwrap();
        assert_eq!(absent.cache_read_input_tokens, None);
        assert_eq!(absent.cache_creation_input_tokens, None);
    }

    #[test]
    fn thinking_content_round_trips_with_signature() {
        let original = MessageContent::Thinking {
            thinking: "step one, then step two".to_string(),
            signature: "ErUBCkYIBRgCIkAe0pAQ==".to_string(),
        };

        let goose = original.to_goose_content().unwrap();
        let GooseMessageContent::Thinking(thinking) = &goose else {
            panic!("expected thinking content");
        };
        assert_eq!(thinking.thinking, "step one, then step two");
        assert_eq!(thinking.signature, "ErUBCkYIBRgCIkAe0pAQ==");

        let round_tripped = MessageContent::from_goose_content(&goose).unwrap();
        let MessageContent::Thinking {
            thinking,
            signature,
        } = round_tripped
        else {
            panic!("expected thinking content");
        };
        assert_eq!(thinking, "step one, then step two");
        assert_eq!(signature, "ErUBCkYIBRgCIkAe0pAQ==");
    }

    #[test]
    fn redacted_thinking_content_round_trips_opaque_data() {
        let original = MessageContent::RedactedThinking {
            data: "EroBCkYIBRgCKkBb0pAQopaque".to_string(),
        };

        let goose = original.to_goose_content().unwrap();
        let GooseMessageContent::RedactedThinking(redacted) = &goose else {
            panic!("expected redacted thinking content");
        };
        assert_eq!(redacted.data, "EroBCkYIBRgCKkBb0pAQopaque");

        let round_tripped = MessageContent::from_goose_content(&goose).unwrap();
        let MessageContent::RedactedThinking { data } = round_tripped else {
            panic!("expected redacted thinking content");
        };
        assert_eq!(data, "EroBCkYIBRgCKkBb0pAQopaque");
    }

    #[test]
    fn tool_request_round_trips_provider_metadata() {
        let original = MessageContent::ToolRequest {
            id: "call_456".to_string(),
            name: "test_tool".to_string(),
            arguments_json: "{}".to_string(),
            provider_metadata_json: Some(
                r#"{"extra_content":{"google":{"thought_signature":"nested_sig_xyz789"}}}"#
                    .to_string(),
            ),
            tool_error_json: None,
        };

        let goose = original.to_goose_content().unwrap();
        let GooseMessageContent::ToolRequest(request) = &goose else {
            panic!("expected tool request");
        };
        assert_eq!(
            request.metadata.as_ref().unwrap()["extra_content"]["google"]["thought_signature"],
            "nested_sig_xyz789"
        );

        let round_tripped = MessageContent::from_goose_content(&goose).unwrap();
        let MessageContent::ToolRequest {
            provider_metadata_json,
            ..
        } = &round_tripped
        else {
            panic!("expected tool request");
        };
        assert_eq!(
            serde_json::from_str::<Value>(provider_metadata_json.as_ref().unwrap()).unwrap(),
            serde_json::json!({"extra_content":{"google":{"thought_signature":"nested_sig_xyz789"}}})
        );

        let messages = convert_messages(vec![ProviderMessage {
            role: MessageRole::Assistant,
            content: vec![round_tripped],
        }])
        .unwrap();
        let spec = goose_providers::formats::openai::format_messages(
            &messages,
            &goose_providers::images::ImageFormat::OpenAi,
        );
        assert_eq!(
            spec[0]["tool_calls"][0]["extra_content"]["google"]["thought_signature"],
            "nested_sig_xyz789"
        );
    }

    #[test]
    fn completion_content_preserves_malformed_tool_requests() {
        let error = rmcp::model::ErrorData {
            code: rmcp::model::ErrorCode::INVALID_REQUEST,
            message: std::borrow::Cow::from(
                "The provided function name was empty; a tool call must name a tool".to_string(),
            ),
            data: None,
        };
        let message = Message::assistant()
            .with_tool_request("call_bad_1", Err(error))
            .with_text("done");

        let content: Vec<MessageContent> = message
            .content
            .iter()
            .filter_map(MessageContent::from_goose_content)
            .collect();

        assert_eq!(
            content.len(),
            2,
            "the failed tool request must not be dropped"
        );
        let MessageContent::ToolRequest {
            id,
            tool_error_json,
            ..
        } = &content[0]
        else {
            panic!("expected tool request");
        };
        assert_eq!(id, "call_bad_1");
        assert!(tool_error_json.is_some());

        let replayed = convert_messages(vec![ProviderMessage {
            role: MessageRole::Assistant,
            content,
        }])
        .unwrap();
        let GooseMessageContent::ToolRequest(request) = &replayed[0].content[0] else {
            panic!("expected tool request");
        };
        let replayed_error = request
            .tool_call
            .as_ref()
            .expect_err("replayed request must stay a failed tool call");
        assert_eq!(replayed_error.code, rmcp::model::ErrorCode::INVALID_REQUEST);
        assert!(replayed_error.message.contains("must name a tool"));
    }

    #[test]
    fn streaming_tool_chunks_carry_provider_metadata() {
        let mut metadata = goose_providers::conversation::message::ProviderMetadata::new();
        metadata.insert(
            "extra_content".to_string(),
            serde_json::json!({"google": {"thought_signature": "stream_sig_abc123"}}),
        );

        let message = Message::assistant().with_tool_request_with_metadata(
            "call_stream_1",
            Ok(CallToolRequestParams::new("test_tool")),
            Some(&metadata),
            None,
        );

        let chunks = message_to_chunks(message);
        let StreamChunk::ToolChunk {
            provider_metadata_json,
            ..
        } = chunks
            .iter()
            .find(|chunk| matches!(chunk, StreamChunk::ToolChunk { .. }))
            .expect("expected a tool chunk")
        else {
            unreachable!()
        };

        assert_eq!(
            serde_json::from_str::<Value>(provider_metadata_json.as_ref().unwrap()).unwrap(),
            serde_json::json!({"extra_content":{"google":{"thought_signature":"stream_sig_abc123"}}})
        );
    }

    #[test]
    fn thinking_blocks_survive_multi_turn_replay() {
        let assistant_turn = ProviderMessage {
            role: MessageRole::Assistant,
            content: vec![
                MessageContent::Thinking {
                    thinking: "the user wants the capital".to_string(),
                    signature: "sig-abc123".to_string(),
                },
                MessageContent::RedactedThinking {
                    data: "opaque-payload".to_string(),
                },
                MessageContent::Text {
                    text: "Paris".to_string(),
                },
            ],
        };

        let history = vec![
            ProviderMessage {
                role: MessageRole::User,
                content: vec![MessageContent::Text {
                    text: "what is the capital of France?".to_string(),
                }],
            },
            assistant_turn,
            ProviderMessage {
                role: MessageRole::User,
                content: vec![MessageContent::Text {
                    text: "and of Spain?".to_string(),
                }],
            },
        ];

        let messages = convert_messages(history).unwrap();
        assert!(matches!(
            messages[1].content[0],
            GooseMessageContent::Thinking(_)
        ));
        assert!(matches!(
            messages[1].content[1],
            GooseMessageContent::RedactedThinking(_)
        ));

        let spec = goose_providers::formats::anthropic::format_messages(&messages);
        let assistant = &spec[1]["content"];
        assert_eq!(assistant[0]["type"], "thinking");
        assert_eq!(assistant[0]["thinking"], "the user wants the capital");
        assert_eq!(assistant[0]["signature"], "sig-abc123");
        assert_eq!(assistant[1]["type"], "redacted_thinking");
        assert_eq!(assistant[1]["data"], "opaque-payload");
        assert!(assistant[1].get("thinking").is_none());
        assert_eq!(assistant[2]["type"], "text");
        assert_eq!(assistant[2]["text"], "Paris");
    }

    #[test]
    fn completion_content_preserves_thinking_for_the_next_turn() {
        let message = Message::assistant()
            .with_thinking("reasoning to replay", "sig-xyz")
            .with_redacted_thinking("opaque-payload")
            .with_text("Madrid");

        let content: Vec<MessageContent> = message
            .content
            .iter()
            .filter_map(MessageContent::from_goose_content)
            .collect();

        assert!(matches!(
            &content[0],
            MessageContent::Thinking { thinking, signature }
                if thinking == "reasoning to replay" && signature == "sig-xyz"
        ));
        assert!(matches!(
            &content[1],
            MessageContent::RedactedThinking { data } if data == "opaque-payload"
        ));

        let replayed = convert_messages(vec![ProviderMessage {
            role: MessageRole::Assistant,
            content,
        }])
        .unwrap();
        assert_eq!(replayed[0].content, message.content);
    }
}
