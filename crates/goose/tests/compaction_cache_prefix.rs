//! The compaction request must replay the last routed request's own prefix
//! (system prompt, tools, projected messages) with the compaction instruction
//! as the final user message, so the provider's prompt cache is reused.

use std::sync::Mutex;

use async_trait::async_trait;
use goose::context_mgmt::{compact_messages, request_header, CompactionResult};
use goose::conversation::message::{Message, MessageContent};
use goose::conversation::{fix_conversation, merge_consecutive_messages_for_request, Conversation};
use goose::providers::base::Provider;
use goose_providers::base::{stream_from_single_message, MessageStream};
use goose_providers::conversation::token_usage::{ProviderUsage, Usage};
use goose_providers::errors::ProviderError;
use goose_providers::model::ModelConfig;
use rmcp::model::{CallToolRequestParams, CallToolResult, ContentBlock, Tool};
use rmcp::object;

const SYSTEM: &str = "You are goose, a careful coding assistant.";

#[derive(Clone)]
struct CapturedRequest {
    model_config: ModelConfig,
    system: String,
    messages: Vec<Message>,
    tools: Vec<Tool>,
}

struct CapturingProvider {
    response: Message,
    usage: Usage,
    captured: Mutex<Vec<CapturedRequest>>,
}

impl CapturingProvider {
    fn new(response: Message) -> Self {
        Self::with_usage(response, Usage::default())
    }

    fn with_usage(response: Message, usage: Usage) -> Self {
        Self {
            response,
            usage,
            captured: Mutex::new(Vec::new()),
        }
    }

    fn requests(&self) -> Vec<CapturedRequest> {
        self.captured.lock().unwrap().clone()
    }
}

#[async_trait]
impl Provider for CapturingProvider {
    fn get_name(&self) -> &str {
        "anthropic"
    }

    async fn stream(
        &self,
        model_config: &ModelConfig,
        system: &str,
        messages: &[Message],
        tools: &[Tool],
    ) -> Result<MessageStream, ProviderError> {
        self.captured.lock().unwrap().push(CapturedRequest {
            model_config: model_config.clone(),
            system: system.to_string(),
            messages: messages.to_vec(),
            tools: tools.to_vec(),
        });
        Ok(stream_from_single_message(
            self.response.clone(),
            ProviderUsage::new("claude-test".to_string(), self.usage),
        ))
    }
}

fn tools() -> Vec<Tool> {
    vec![Tool::new(
        "read_file",
        "Read a file from disk",
        object!({
            "type": "object",
            "properties": { "path": { "type": "string" } }
        }),
    )]
}

fn conversation() -> Conversation {
    Conversation::new_unvalidated(vec![
        Message::user().with_text("What does the main entrypoint do?"),
        Message::assistant().with_tool_request(
            "call_1",
            Ok(CallToolRequestParams::new("read_file")
                .with_arguments(object!({ "path": "src/main.rs" }))),
        ),
        Message::user().with_tool_response(
            "call_1",
            Ok(CallToolResult::success(vec![ContentBlock::text(
                "fn main() { run(); }",
            )])),
        ),
        Message::assistant().with_text("It calls `run()`."),
    ])
}

/// The projection every request goes through in
/// `stream_response_from_provider` before any formatter.
fn provider_view(messages: &[Message]) -> Vec<Message> {
    let projected =
        Conversation::new_unvalidated(messages.iter().cloned()).agent_visible_messages();
    let (fixed, _) = fix_conversation(Conversation::new_unvalidated(projected));
    merge_consecutive_messages_for_request(fixed.messages().clone())
}

fn main_model_config() -> ModelConfig {
    ModelConfig::new("claude-test").with_context_limit(Some(200_000))
}

fn record_header(session_id: &str) {
    request_header::record(
        session_id,
        request_header::RequestHeader {
            system_prompt: SYSTEM.to_string(),
            tools: tools(),
            toolshim_tools: Vec::new(),
        },
    );
}

/// A toolshim session's routed requests carry no provider-native tools; the
/// real definitions ride along for response interpretation.
fn record_toolshim_header(session_id: &str) {
    request_header::record(
        session_id,
        request_header::RequestHeader {
            system_prompt: SYSTEM.to_string(),
            tools: Vec::new(),
            toolshim_tools: tools(),
        },
    );
}

async fn run_compaction(
    provider: &CapturingProvider,
    session_id: &str,
    conversation: &Conversation,
) -> anyhow::Result<CompactionResult> {
    run_compaction_with_config(provider, &main_model_config(), session_id, conversation).await
}

async fn run_compaction_with_config(
    provider: &CapturingProvider,
    model_config: &ModelConfig,
    session_id: &str,
    conversation: &Conversation,
) -> anyhow::Result<CompactionResult> {
    compact_messages(provider, model_config, session_id, conversation, true).await
}

fn assert_prefix_shape(request: &CapturedRequest, conversation: &Conversation) {
    assert!(
        !request.model_config.prompt_cache_disabled(),
        "the compaction request must keep prompt-cache breakpoints"
    );
    let expected_prefix = provider_view(conversation.messages());
    assert_eq!(&request.messages[..expected_prefix.len()], &expected_prefix);
    assert_eq!(request.messages.len(), expected_prefix.len() + 2);
    let instruction = request.messages.last().unwrap();
    assert_eq!(instruction.role, rmcp::model::Role::User);
    assert!(instruction
        .as_concat_text()
        .contains("distill the conversation so far"));
}

#[tokio::test]
async fn compaction_replays_the_header_and_rejects_a_non_summary_after_one_retry() {
    let session_id = "prefix-session-rejected";
    record_header(session_id);

    let response = Message::assistant().with_tool_request(
        "call_2",
        Ok(CallToolRequestParams::new("read_file")
            .with_arguments(object!({ "path": "src/lib.rs" }))),
    );
    let provider = CapturingProvider::with_usage(response, Usage::new(Some(100), Some(10), None));
    let conversation = conversation();
    let error = match run_compaction(&provider, session_id, &conversation).await {
        Ok(_) => panic!("a tool-calling summary response must fail compaction"),
        Err(error) => error,
    };

    let requests = provider.requests();
    assert_eq!(requests.len(), 2, "one corrective retry, then fail");
    assert_eq!(requests[0].system, SYSTEM);
    assert_eq!(requests[0].tools.len(), 1);
    assert_prefix_shape(&requests[0], &conversation);

    let retry = &requests[1];
    let (paired_response, follow_up) = (
        &retry.messages[retry.messages.len() - 2],
        retry.messages.last().unwrap(),
    );
    assert_eq!(paired_response.role, rmcp::model::Role::Assistant);
    assert!(follow_up.as_concat_text().contains("without calling tools"));
    assert_eq!(
        &retry.messages[..requests[0].messages.len()],
        &requests[0].messages[..],
        "the retry must extend the rejected request's prefix"
    );

    let failure = error
        .downcast_ref::<goose_context_management::CompactionFailure>()
        .expect("the error must carry the compaction failure");
    assert_eq!(
        failure
            .billed_usage
            .iter()
            .map(|usage| usage.usage.input_tokens)
            .collect::<Vec<_>>(),
        vec![Some(100), Some(100)],
        "each rejected call's billed usage must be reported"
    );
}

fn has_tool_content(messages: &[Message]) -> bool {
    messages.iter().any(|message| {
        message.content.iter().any(|content| {
            matches!(
                content,
                MessageContent::ToolRequest(_) | MessageContent::ToolResponse(_)
            )
        })
    })
}

fn concat_request_text(request: &CapturedRequest) -> String {
    request
        .messages
        .iter()
        .map(|message| message.as_concat_text())
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn toolshim_summarizer_responses_are_interpreted_before_acceptance() {
    let model_config = main_model_config().with_toolshim(true);

    let session_id = "prefix-session-toolshim-rejected";
    record_toolshim_header(session_id);
    let tool_call = Message::assistant()
        .with_text(r#"{"name": "read_file", "arguments": {"path": "src/lib.rs"}}"#);
    let provider = CapturingProvider::new(tool_call);
    run_compaction_with_config(&provider, &model_config, session_id, &conversation())
        .await
        .err()
        .expect("a textual tool call must be rejected as a summary");
    let requests = provider.requests();
    assert_eq!(requests.len(), 2, "one corrective retry, then fail");
    assert!(
        concat_request_text(&requests[1]).contains("was not executed"),
        "the stub tool result must reach the model in text form"
    );
    assert!(!has_tool_content(&requests[1].messages));

    let session_id = "prefix-session-toolshim-quoting";
    record_toolshim_header(session_id);
    let quoting_summary = Message::assistant().with_text(concat!(
        "The session ran Using tool: {\"name\": \"read_file\", ",
        "\"arguments\": {\"path\": \"src/main.rs\"}} and read the entrypoint.\n\n",
        "```json\n{\"user_intent\": [\"understand the main entrypoint\"], ",
        "\"current_work\": \"done\"}\n```",
    ));
    let provider = CapturingProvider::new(quoting_summary);
    run_compaction_with_config(&provider, &model_config, session_id, &conversation())
        .await
        .expect("a structured summary quoting shim-protocol history must not be rejected");
    assert_eq!(provider.requests().len(), 1);
}
