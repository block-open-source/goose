use anyhow::Result;
use async_trait::async_trait;
use futures::StreamExt;
use goose::agents::extension::ExtensionConfig;
use goose::agents::{Agent, AgentEvent, SessionConfig};
use goose::config::GooseMode;
use goose::conversation::message::{Message, MessageContent};
use goose::conversation::Conversation;
use goose::permission::Permission;
use goose::providers::base::{
    stream_from_single_message, MessageStream, Provider, ProviderDef, ProviderMetadata,
};
use goose::recipe::Response;
use goose::session::session_manager::SessionType;
use goose::session::Session;
use goose_providers::conversation::token_usage::{ProviderUsage, Usage};
use goose_providers::errors::ProviderError;
use goose_providers::model::ModelConfig;
use rmcp::model::{CallToolRequestParams, Tool};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

struct MockCompactionProvider {
    /// Tracks whether compaction has occurred (for context limit recovery case)
    has_compacted: Arc<AtomicBool>,
    manages_own_context: bool,
    /// When > 0, non-compaction replies interleave a tool request with the
    /// reply text for this many round-trips before finishing the turn.
    tool_loop_rounds: usize,
    /// Whether loop replies carry `long_tool_call` padding (15k tokens in the
    /// input and output accounting), so each round-trip grows the context.
    tool_loop_padding: bool,
    /// Whether loop replies request the developer `shell` tool (whose real
    /// truncated output grows the context) instead of the fictional padding.
    tool_result_padding: bool,
    /// How many `shell` calls a tool-result round-trip requests.
    tool_result_calls: usize,
    /// The command the `shell` calls run.
    tool_result_command: String,
    /// When set, every real summarization request fails, so mid-turn
    /// compaction is attempted but cannot succeed.
    fail_summarizations: bool,
    /// When set, the reply to the kickoff calls the final-output tool beside
    /// `long_tool_call` padding, so usage crosses the threshold exactly when
    /// the recipe result is ready to deliver.
    final_output_reply: bool,
    /// When set, the first regular reply carries no content while still
    /// reporting usage, so the loop drops the response and the turn's usage
    /// has no assistant message to attach to.
    empty_first_reply: bool,
    /// Fictional token cost of the system prompt in input accounting. Lowered
    /// for scenarios that grow the context through real tool output: there the
    /// reported usage must stay under the threshold so only a conversation
    /// recount can see the growth.
    system_input_tokens: i32,
    /// Context limit the provider reports to the agent. The streaming wall
    /// below stays at 20k so a provider rejection stays distinguishable from
    /// the auto-compact threshold.
    context_limit: usize,
    context_limit_rejections: Arc<AtomicUsize>,
}

/// A command whose stdout (~120k characters) truncates to a ~10k-character
/// (≈5k-token) preview, growing the conversation per call.
fn awk_command() -> String {
    "awk 'BEGIN{for(i=0;i<60000;i++)printf \"x \"}'".to_string()
}

/// A command whose stdout (~2k characters, ≈1k tokens) lands whole — under
/// the shell's truncation limits — so parallel calls grow the conversation
/// by a bounded, sub-threshold amount each.
fn small_output_command() -> String {
    "awk 'BEGIN{for(i=0;i<1000;i++)printf \"x \"}'".to_string()
}

impl MockCompactionProvider {
    fn new() -> Self {
        Self {
            has_compacted: Arc::new(AtomicBool::new(false)),
            manages_own_context: false,
            tool_loop_rounds: 0,
            tool_loop_padding: false,
            tool_result_padding: false,
            tool_result_calls: 2,
            tool_result_command: awk_command(),
            fail_summarizations: false,
            final_output_reply: false,
            empty_first_reply: false,
            system_input_tokens: 6_000,
            context_limit: 128_000,
            context_limit_rejections: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn context_owning() -> Self {
        Self {
            manages_own_context: true,
            ..Self::new()
        }
    }

    /// A provider that keeps calling tools, inflating the context by 15k
    /// tokens per round-trip. Reports a 16k context limit, so the auto-compact
    /// threshold (12.8k) sits below the 20k streaming wall.
    fn looping() -> Self {
        Self {
            tool_loop_rounds: 2,
            tool_loop_padding: true,
            context_limit: 16_000,
            ..Self::new()
        }
    }

    /// One round-trip whose two `shell` calls land real truncated output
    /// (~10k characters each, ≈5k tokens a pair) in the conversation. The
    /// lean input accounting keeps reported usage far under the 8k threshold,
    /// so only the conversation the tool responses grew can cross it.
    fn result_looping() -> Self {
        Self {
            tool_loop_rounds: 1,
            tool_result_padding: true,
            system_input_tokens: 500,
            context_limit: 10_000,
            ..Self::new()
        }
    }

    /// One round-trip whose three `shell` calls each land ~1k tokens of real
    /// output. Against the 8k threshold, the reported usage (~6.2k, dominated
    /// by the 6k system overhead) plus any single result stays under, and so
    /// does the conversation alone (~3.5k): only the sum of the parallel
    /// results added to the baseline crosses. The legacy loop persists the
    /// batch as alternating request/response pairs, so a recount that stops
    /// at the newest assistant message — a synthetic split of the same
    /// response — sees only the final result and misses the crossing.
    fn parallel_baseline_growth() -> Self {
        Self {
            tool_loop_rounds: 1,
            tool_result_padding: true,
            tool_result_calls: 3,
            tool_result_command: small_output_command(),
            system_input_tokens: 500,
            context_limit: 10_000,
            ..Self::new()
        }
    }

    /// One round-trip with a single `shell` call, against a 10k limit whose
    /// threshold sits at 8k. Reported usage (~6.5k: the 6k system overhead
    /// dominates) and the conversation alone (~5.5k) each stay under the
    /// threshold, so only a check that adds the unsent tool result to the
    /// reported baseline can see the crossing.
    fn baseline_growth() -> Self {
        Self {
            tool_loop_rounds: 1,
            tool_result_padding: true,
            tool_result_calls: 1,
            system_input_tokens: 500,
            context_limit: 10_000,
            ..Self::new()
        }
    }

    /// One `sleep`-ing `shell` call beside reply padding that drives reported
    /// usage past the 12.8k threshold, so an over-threshold session reaches
    /// the mid-turn check while the tool is still executing — the window a
    /// client cancellation lands in.
    fn cancellable() -> Self {
        Self {
            tool_loop_rounds: 1,
            tool_loop_padding: true,
            tool_result_padding: true,
            tool_result_calls: 1,
            tool_result_command: "sleep 1".to_string(),
            context_limit: 16_000,
            ..Self::new()
        }
    }

    /// Same growth as `result_looping`, but every summarization request
    /// fails, so the mid-turn compaction attempt cannot succeed.
    fn failing_summarization() -> Self {
        Self {
            fail_summarizations: true,
            ..Self::result_looping()
        }
    }

    /// Replies to the kickoff with `long_tool_call` padding (which drives
    /// reported usage past the 12.8k threshold) beside a final-output tool
    /// call, so the threshold is crossed exactly when the recipe result is
    /// ready to deliver.
    fn final_output() -> Self {
        Self {
            final_output_reply: true,
            tool_loop_padding: true,
            context_limit: 16_000,
            ..Self::new()
        }
    }

    /// Same tool loop, but the provider manages its own context: the padding
    /// stays small so the loop never approaches the 20k wall.
    fn context_owning_looping() -> Self {
        Self {
            manages_own_context: true,
            tool_loop_rounds: 2,
            tool_loop_padding: false,
            context_limit: 16_000,
            ..Self::new()
        }
    }

    /// Plain text-only replies against a 10k limit (8k threshold): no tool
    /// calls, so growth can only come from the conversation itself —
    /// preloaded history or a drained steer — never from this turn's
    /// round-trips.
    fn conversational() -> Self {
        Self {
            context_limit: 10_000,
            ..Self::new()
        }
    }

    /// Same conversational replies, but the first response is content-free
    /// while still reporting usage: the loop drops the empty assistant
    /// message, so only a preserved usage boundary can keep the drained
    /// steer's re-check from sweeping the already-sent kickoff back in as
    /// unsent growth.
    fn empty_then_conversational() -> Self {
        Self {
            empty_first_reply: true,
            ..Self::conversational()
        }
    }

    fn context_limit_rejections(&self) -> usize {
        self.context_limit_rejections.load(Ordering::SeqCst)
    }

    /// Whether the kickoff still awaits its final-output reply: the
    /// final-output mode answers the first request and nothing later.
    fn final_output_reply_due(&self, messages: &[Message]) -> bool {
        self.final_output_reply
            && messages
                .iter()
                .any(|msg| msg.as_concat_text().contains("keep processing each result"))
            && !messages
                .iter()
                .any(|msg| msg.as_concat_text().contains("Your context was compacted"))
    }

    /// The round-trip a loop reply belongs to, based on the tool responses
    /// already in the conversation. `None` once the loop is done or the
    /// conversation was compacted (the continuation text ends the turn).
    fn loop_reply_round(&self, messages: &[Message]) -> Option<usize> {
        if self.tool_loop_rounds == 0 {
            return None;
        }
        if messages
            .iter()
            .any(|msg| msg.as_concat_text().contains("Your context was compacted"))
        {
            return None;
        }
        let done = messages
            .iter()
            .filter(|msg| {
                msg.content
                    .iter()
                    .any(|c| matches!(c, MessageContent::ToolResponse(_)))
            })
            .count();
        (done < self.tool_loop_rounds).then_some(done + 1)
    }

    /// Calculate input tokens based on system prompt and messages
    /// Simulates realistic token counts for different scenarios
    fn calculate_input_tokens(&self, system_prompt: &str, messages: &[Message]) -> i32 {
        // Check if this is a compaction call
        let is_compaction_call = messages.len() == 1
            && messages[0].content.iter().any(|c| {
                if let MessageContent::Text(text) = c {
                    text.text.to_lowercase().contains("summarize")
                } else {
                    false
                }
            });

        if is_compaction_call {
            // For compaction: system prompt length is a good proxy for conversation size
            self.system_input_tokens.max(6_000) + (system_prompt.len() as i32 / 4).max(400)
        } else {
            // Regular call: system prompt + messages
            let system_tokens = if system_prompt.is_empty() { 0 } else { 6000 };

            let message_tokens: i32 = messages
                .iter()
                .map(|msg| {
                    let mut tokens = 100;
                    for content in &msg.content {
                        let serialized = match content {
                            MessageContent::Text(text) => text.text.clone(),
                            MessageContent::Thinking(thinking) => thinking.thinking.clone(),
                            _ => String::new(),
                        };
                        if serialized.contains("long_tool_call") {
                            tokens += 15000;
                        }
                    }
                    tokens
                })
                .sum();

            system_tokens + message_tokens
        }
    }

    /// Calculate output tokens based on response type
    fn calculate_output_tokens(&self, is_compaction: bool, messages: &[Message]) -> i32 {
        if is_compaction {
            // Compaction produces a compact summary
            200
        } else if (self.loop_reply_round(messages).is_some() && self.tool_loop_padding)
            || self.final_output_reply_due(messages)
        {
            // Replies that carry the fictional padding payload
            15_000
        } else {
            // Regular responses vary by content
            let has_hello = messages.iter().any(|msg| {
                msg.content.iter().any(|c| {
                    if let MessageContent::Text(text) = c {
                        text.text.to_lowercase().contains("hello")
                    } else {
                        false
                    }
                })
            });

            if has_hello {
                50 // Simple greeting response
            } else {
                100 // Default response
            }
        }
    }
}

#[async_trait]
impl Provider for MockCompactionProvider {
    fn manages_own_context(&self) -> bool {
        self.manages_own_context
    }

    async fn get_context_limit(&self, _model: &str, _override_limit: Option<usize>) -> usize {
        self.context_limit
    }

    async fn stream(
        &self,
        _model_config: &ModelConfig,
        system_prompt: &str,
        messages: &[Message],
        _tools: &[Tool],
    ) -> Result<MessageStream, ProviderError> {
        // Any "summarize" text marks the request as compaction-related: the
        // continuation prompt after a compaction also matches, which existing
        // tests rely on for their token accounting.
        let is_compaction = messages.iter().any(|msg| {
            msg.content.iter().any(|content| {
                if let MessageContent::Text(text) = content {
                    text.text.to_lowercase().contains("summarize")
                } else {
                    false
                }
            })
        });
        // Only a real summarization request is a lone "summarize" message
        let is_summarization_request = messages.len() == 1 && is_compaction;

        // Calculate realistic token counts based on actual content
        let input_tokens = self.calculate_input_tokens(system_prompt, messages);
        let loop_round = self.loop_reply_round(messages);
        let loop_finished = self.tool_loop_rounds > 0 && loop_round.is_none();
        let output_tokens = self.calculate_output_tokens(is_compaction, messages);

        // Simulate context limit: if input > 20k tokens and we haven't compacted yet, fail
        const CONTEXT_LIMIT: i32 = 20000;
        if !is_compaction
            && input_tokens > CONTEXT_LIMIT
            && !self.has_compacted.load(Ordering::SeqCst)
        {
            self.context_limit_rejections.fetch_add(1, Ordering::SeqCst);
            return Err(ProviderError::ContextLengthExceeded(format!(
                "Context limit exceeded: {} > {}",
                input_tokens, CONTEXT_LIMIT
            )));
        }

        // If this is a compaction call, mark that we've compacted
        if is_compaction {
            self.has_compacted.store(true, Ordering::SeqCst);
        }

        if is_summarization_request && self.fail_summarizations {
            return Err(ProviderError::ServerError(
                "summarization unavailable".to_string(),
            ));
        }

        // Generate response
        let message = if is_summarization_request {
            Message::assistant().with_text("<mock summary of conversation>")
        } else if self.empty_first_reply
            && !is_compaction
            // The kickoff is answered before the steer lands; once the
            // drained steer rides in the request, replies are normal. The
            // loop merges consecutive user messages, so the message count
            // cannot tell the two apart.
            && !messages
                .iter()
                .any(|msg| msg.as_concat_text().contains("steered onward"))
        {
            // A content-free response with usage: the loop drops it, so the
            // turn's usage has no assistant message to attach to.
            Message::assistant()
        } else if self.final_output_reply_due(messages) {
            // The padding rides in the thinking block (counted in the output
            // accounting above), so reported usage crosses the threshold in
            // the same round-trip that collects the recipe result.
            let mut arguments = serde_json::Map::new();
            arguments.insert(
                "result".to_string(),
                serde_json::Value::String("42".to_string()),
            );
            Message::assistant()
                .with_thinking(format!("long_tool_call final: {}", "x".repeat(600)), "")
                .with_tool_request(
                    "final_output_call",
                    Ok(CallToolRequestParams::new("recipe__final_output")
                        .with_arguments(arguments)),
                )
        } else if let Some(round) = loop_round {
            // The legacy loop does not persist assistant text next to a tool
            // call, so the padding rides in the thinking block, which is
            // carried onto the persisted tool-call message.
            let reply = Message::assistant().with_text(format!("tool loop round {round}"));
            let reply = if self.tool_loop_padding {
                reply.with_thinking(
                    format!("long_tool_call round {round}: {}", "x".repeat(600)),
                    "",
                )
            } else {
                reply
            };
            if self.tool_result_padding {
                let mut arguments = serde_json::Map::new();
                arguments.insert(
                    "command".to_string(),
                    serde_json::Value::String(self.tool_result_command.clone()),
                );
                let mut reply = reply;
                for call in 1..=self.tool_result_calls {
                    reply = reply.with_tool_request(
                        format!("loop_call_{call}"),
                        Ok(CallToolRequestParams::new("shell").with_arguments(arguments.clone())),
                    );
                }
                reply
            } else {
                reply.with_tool_request(
                    format!("loop_call_{round}"),
                    Ok(CallToolRequestParams::new("echo_tool")),
                )
            }
        } else if messages
            .iter()
            .any(|msg| msg.as_concat_text().contains("Your context was compacted"))
            || loop_finished
        {
            Message::assistant().with_text("done after the tool loop")
        } else if is_compaction {
            Message::assistant().with_text("<mock summary of conversation>")
        } else {
            let response_text = if messages.iter().any(|msg| {
                msg.content.iter().any(|c| {
                    if let MessageContent::Text(text) = c {
                        text.text.to_lowercase().contains("hello")
                    } else {
                        false
                    }
                })
            }) {
                "Hi there! How can I help you?"
            } else {
                "This is a mock response."
            };
            Message::assistant().with_text(response_text)
        };

        let usage = ProviderUsage::new(
            "mock-model".to_string(),
            Usage::new(
                Some(input_tokens),
                Some(output_tokens),
                Some(input_tokens + output_tokens),
            ),
        );

        Ok(stream_from_single_message(message, usage))
    }

    fn get_name(&self) -> &str {
        "mock-compaction"
    }
}

impl goose::providers::base::ProviderDescriptor for MockCompactionProvider {
    fn metadata() -> ProviderMetadata {
        ProviderMetadata {
            name: "mock".to_string(),
            display_name: "Mock Compaction Provider".to_string(),
            description: "Mock provider for compaction testing".to_string(),
            default_model: "mock-model".to_string(),
            known_models: vec![],
            model_doc_link: "".to_string(),
            config_keys: vec![],
            setup_steps: vec![],
            setup: None,
            deprecated: None,
        }
    }
}

impl ProviderDef for MockCompactionProvider {
    type Provider = Self;

    fn from_env(
        _extensions: Vec<goose::config::ExtensionConfig>,
        _tls_config: Option<goose::providers::api_client::TlsConfig>,
    ) -> futures::future::BoxFuture<'static, anyhow::Result<Self>> {
        Box::pin(async { Ok(Self::new()) })
    }
}

/// Helper: Set up a test session with initial messages and token counts
async fn setup_test_session(
    agent: &Agent,
    temp_dir: &TempDir,
    session_name: &str,
    messages: Vec<Message>,
) -> Result<Session> {
    let session = agent
        .config
        .session_manager
        .create_session(
            temp_dir.path().to_path_buf(),
            session_name.to_string(),
            SessionType::Hidden,
            GooseMode::default(),
        )
        .await?;

    let conversation = Conversation::new_unvalidated(messages);
    agent
        .config
        .session_manager
        .replace_conversation(&session.id, &conversation)
        .await?;

    // Set initial token counts
    agent
        .config
        .session_manager
        .update(&session.id)
        .usage(Usage::new(Some(600), Some(400), Some(1000)))
        .accumulated_usage(Usage::new(Some(600), Some(400), Some(1000)))
        .apply()
        .await?;

    Ok(session)
}

#[tokio::test]
async fn context_owning_provider_rejects_clear_and_compact_without_changing_session() -> Result<()>
{
    let temp_dir = TempDir::new()?;
    let agent = Agent::new();
    let messages = vec![
        Message::user().with_text("Remember this"),
        Message::assistant().with_text("I will"),
    ];
    let session = setup_test_session(
        &agent,
        &temp_dir,
        "context-owning-provider",
        messages.clone(),
    )
    .await?;
    let before = agent
        .config
        .session_manager
        .get_session(&session.id, true)
        .await?;
    let conversation_before = before.conversation.unwrap();
    let usage_before = before.usage;
    let provider = Arc::new(MockCompactionProvider::context_owning());
    agent
        .update_provider(provider, ModelConfig::new("mock-model"), &session.id)
        .await?;

    for command in ["clear", "compact"] {
        let error = agent
            .execute_command(&format!("/{command}"), &session.id)
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            format!(
                "/{command} is not available for provider 'mock-compaction' because it manages its own conversation context"
            )
        );
    }

    let unchanged = agent
        .config
        .session_manager
        .get_session(&session.id, true)
        .await?;
    assert_eq!(unchanged.conversation.unwrap(), conversation_before);
    assert_eq!(unchanged.usage, usage_before);

    Ok(())
}

/// Helper: Assert conversation has been compacted with proper message visibility
fn assert_conversation_compacted(conversation: &Conversation) {
    let messages = conversation.messages();
    assert!(!messages.is_empty(), "Conversation should not be empty");

    // Find the summary message (contains "mock summary")
    let summary_index = messages
        .iter()
        .position(|msg| {
            msg.content.iter().any(|content| {
                if let MessageContent::Text(text) = content {
                    text.text.contains("mock summary")
                } else {
                    false
                }
            })
        })
        .expect("Conversation should contain the summary message");

    let summary_msg = &messages[summary_index];

    // Assert summary message visibility
    assert!(
        summary_msg.is_agent_visible(),
        "Summary message should be agent visible"
    );
    assert!(
        !summary_msg.is_user_visible(),
        "Summary message should NOT be user visible"
    );

    // Check messages BEFORE the summary (the compacted original messages)
    // These should be made agent-invisible
    for (idx, msg) in messages.iter().enumerate() {
        if idx < summary_index {
            // Old messages before summary: agent can't see them
            assert!(
                !msg.is_agent_visible(),
                "Message before summary at index {} should be agent-invisible",
                idx
            );
        }
    }

    // Check for continuation message after summary
    // (Should exist and be agent-only)
    if summary_index + 1 < messages.len() {
        let continuation_msg = &messages[summary_index + 1];
        // Continuation message should contain instructions about not mentioning summary
        let has_continuation_text = continuation_msg.content.iter().any(|content| {
            if let MessageContent::Text(text) = content {
                text.text.contains("previous message contains a summary")
                    || text.text.contains("summarization occurred")
            } else {
                false
            }
        });

        if has_continuation_text {
            assert!(
                continuation_msg.is_agent_visible(),
                "Continuation message should be agent visible"
            );
            assert!(
                !continuation_msg.is_user_visible(),
                "Continuation message should NOT be user visible"
            );
        }
    }

    // The projected replay of the preserved user message is agent-only. Any
    // ordinary messages appended after it should remain visible to both sides.
    let continuation_end = summary_index + 2;
    for (idx, msg) in messages.iter().enumerate() {
        if idx >= continuation_end {
            assert!(
                msg.is_agent_visible(),
                "Message after compaction at index {} should be agent visible",
                idx
            );
            if msg.is_turn_context() {
                assert!(
                    !msg.is_user_visible(),
                    "Carried turn-context event should be user-invisible"
                );
            } else if idx == continuation_end && matches!(msg.role, rmcp::model::Role::User) {
                assert!(
                    !msg.is_user_visible(),
                    "Projected preserved user message should be user-invisible"
                );
            } else {
                assert!(
                    msg.is_user_visible(),
                    "Ordinary message after compaction at index {} should be user visible",
                    idx
                );
            }
        }
    }
}

#[tokio::test]
async fn test_manual_compaction_updates_token_counts_and_conversation() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let agent = Agent::new();

    // Setup session with initial messages
    // Each message ~100 tokens, so 4 messages = ~400 tokens in conversation
    let messages = vec![
        Message::user().with_text("Hello, can you help me with something?"),
        Message::assistant().with_text("Of course! What do you need help with?"),
        Message::user().with_text("I need to understand how compaction works."),
        Message::assistant()
            .with_text("Compaction is a process that summarizes conversation history."),
    ];

    let session = setup_test_session(&agent, &temp_dir, "manual-compact-test", messages).await?;

    // Setup mock provider
    let provider = Arc::new(MockCompactionProvider::new());
    agent
        .update_provider(provider, ModelConfig::new("mock-model"), &session.id)
        .await?;

    // Execute manual compaction
    let result = agent.execute_command("/compact", &session.id).await?;
    assert!(result.is_some(), "Compaction should return a result");

    // Verify token counts
    let updated_session = agent
        .config
        .session_manager
        .get_session(&session.id, true)
        .await?;

    // Expected token calculation for compaction:
    // During compaction, the 4 messages are embedded in the system prompt template
    // - Input: system prompt with embedded conversation + "Please summarize" message
    // - Output: summary (200 tokens)
    //
    // From mock provider calculation:
    // - System prompt (with 4 embedded messages): varies based on template + content
    // - Single "summarize" message: 100 tokens
    // - Total input observed: ~6100 tokens
    //
    // After compaction the baseline is the estimated retained conversation
    // (summary + continuation), not the provider-reported output count
    let input_after = updated_session
        .usage
        .input_tokens
        .expect("Input tokens should be set after compaction");
    assert!(
        input_after > 0 && input_after < 200,
        "Input tokens should be the estimated retained context (smaller than the mock's claimed 200 output tokens). Got: {}",
        input_after
    );
    assert_eq!(
        updated_session.usage.output_tokens, None,
        "Output tokens should be None after compaction (no new assistant output)"
    );
    assert_eq!(
        updated_session.usage.total_tokens,
        Some(input_after),
        "Total should equal input after compaction"
    );

    // Accumulated tokens increased by the compaction cost
    // Initial: 1000
    // Compaction input: ~6700 (system 6000 + compaction prompt + 4 messages;
    // the mock derives input tokens from the rendered prompt length, so the
    // band must absorb compaction.md wording changes)
    // Compaction output: 200
    let accumulated = updated_session.accumulated_usage.total_tokens.unwrap();
    assert!(
        (7300..=8600).contains(&accumulated),
        "Accumulated should be ~7900 (1000 initial + ~6700 input + 200 output). Got: {}",
        accumulated
    );

    // Verify conversation has been compacted
    let compacted_conversation = updated_session
        .conversation
        .expect("Session should have conversation");

    assert_conversation_compacted(&compacted_conversation);

    Ok(())
}

#[tokio::test]
async fn test_auto_compaction_during_reply() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let agent = Agent::new();

    // Setup session with many messages to have substantial context
    // 20 exchanges = 40 messages * 100 tokens = ~4000 tokens in conversation
    let mut messages = vec![];
    for i in 0..20 {
        messages.push(Message::user().with_text(format!("User message {}", i)));
        messages.push(Message::assistant().with_text(format!("Assistant response {}", i)));
    }

    let session = setup_test_session(&agent, &temp_dir, "auto-compact-test", messages).await?;

    // Capture initial context size before triggering reply
    // Should be: system (6000) + 40 messages (4000) = ~10000 tokens
    let initial_session = agent
        .config
        .session_manager
        .get_session(&session.id, true)
        .await?;
    let initial_input_tokens = initial_session.usage.input_tokens.unwrap_or(0);

    // Setup mock provider (no context limit enforcement)
    let provider = Arc::new(MockCompactionProvider::new());
    agent
        .update_provider(provider, ModelConfig::new("mock-model"), &session.id)
        .await?;

    // Trigger a reply
    // Expected tokens for reply:
    // - Input: system (6000) + 40 messages (4000) + new user message (100) = 10100 tokens
    // - Output: regular response (100 tokens)
    let user_message = Message::user().with_text("Tell me more about compaction");

    let session_config = SessionConfig {
        id: session.id.clone(),
        schedule_id: None,
        max_turns: None,
        retry_config: None,
    };

    let reply_stream = agent.reply(user_message, session_config, None).await?;
    tokio::pin!(reply_stream);

    // Track compaction and context size changes
    let mut compaction_occurred = false;
    let mut input_tokens_after_compaction: Option<i32> = None;

    while let Some(event_result) = reply_stream.next().await {
        match event_result {
            Ok(AgentEvent::HistoryReplaced(_)) => {
                compaction_occurred = true;

                // Capture the input tokens immediately after compaction
                let session_after_compact = agent
                    .config
                    .session_manager
                    .get_session(&session.id, true)
                    .await?;
                input_tokens_after_compaction = session_after_compact.usage.input_tokens;
            }
            Ok(_) => {}
            Err(e) => return Err(e),
        }
    }

    let updated_session = agent
        .config
        .session_manager
        .get_session(&session.id, true)
        .await?;

    if compaction_occurred {
        // Verify that current input context decreased after compaction
        let tokens_after =
            input_tokens_after_compaction.expect("Should have captured tokens after compaction");

        // Before compaction: system (6000) + 40 messages (4000) = 10,000 tokens
        // After compaction: only the summary (200 tokens) - this becomes the new input
        assert!(
            tokens_after < initial_input_tokens,
            "Input tokens should decrease after compaction. Before: {}, After: {}",
            initial_input_tokens,
            tokens_after
        );

        // After compaction, input should be exactly the summary: 200 tokens
        assert_eq!(
            tokens_after, 200,
            "Input tokens after compaction should be exactly 200 (summary). Got: {}",
            tokens_after
        );

        // After the subsequent reply, the current window includes:
        // - system (6000) + summary (200) + new user message (100) + reply (100) = 6400
        let final_input = updated_session.usage.input_tokens.unwrap();
        let final_output = updated_session.usage.output_tokens.unwrap();
        let final_total = updated_session.usage.total_tokens.unwrap();

        assert!(
            final_input >= 6000,
            "Final input should include at least system prompt (6000). Got: {}",
            final_input
        );
        assert_eq!(
            final_output, 100,
            "Final output should be 100 tokens (default response). Got: {}",
            final_output
        );
        assert_eq!(
            final_total,
            final_input + final_output,
            "Final total should equal input + output"
        );

        // Accumulated tokens should include:
        // - Initial: 1000
        // - Compaction: ~10,400 input + 200 output = 10,600
        // - Reply: ~6,300 input + 100 output = 6,400
        // Total: 1000 + 10,600 + 6,400 = 18,000
        let accumulated = updated_session.accumulated_usage.total_tokens.unwrap();
        assert!(
            (17000..=19000).contains(&accumulated),
            "Accumulated should be ~18,000 (initial + compaction + reply). Got: {}",
            accumulated
        );
    } else {
        // If no compaction, accumulated should include reply cost
        // - Initial: 1000
        // - Reply: system (6000) + 40 messages (4000) + new message (100) = 10,100 input
        // - Reply output: 100
        // Total: 1000 + 10,100 + 100 = 11,200
        let accumulated = updated_session.accumulated_usage.total_tokens.unwrap();
        assert!(
            (11000..=11500).contains(&accumulated),
            "Accumulated should be ~11,200 (initial + reply). Got: {}",
            accumulated
        );

        // Current window should be: 10,100 input + 100 output = 10,200
        let final_input = updated_session.usage.input_tokens.unwrap();
        let final_output = updated_session.usage.output_tokens.unwrap();

        assert!(
            (10000..=10500).contains(&final_input),
            "Input should be ~10,100. Got: {}",
            final_input
        );
        assert_eq!(
            final_output, 100,
            "Output should be 100. Got: {}",
            final_output
        );
    }

    Ok(())
}

#[tokio::test]
async fn test_context_limit_recovery_compaction() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let agent = Agent::new();

    // Setup session with messages that will push context over the limit
    // Each message = 100 tokens, but we'll add a large one
    let messages = vec![
        Message::user().with_text("Hello"),
        Message::assistant().with_text("Hi there"),
        Message::user().with_text("Can you process this long_tool_call result?"),
        Message::assistant().with_text("Processing..."),
    ];
    // Token calculation:
    // - 3 regular messages: 300 tokens
    // - 1 message with "long_tool_call": 100 + 15000 = 15100 tokens
    // - Total conversation: ~15400 tokens
    // - With system prompt (6000): 21400 tokens

    let session = setup_test_session(&agent, &temp_dir, "context-limit-test", messages).await?;

    // Setup mock provider with context limit of 20000 tokens
    // Initial context (6000 system + 15400 messages = 21400) exceeds this limit
    let provider = Arc::new(MockCompactionProvider::new());
    agent
        .update_provider(provider, ModelConfig::new("mock-model"), &session.id)
        .await?;

    // Try to send a message - should trigger context limit, then recover via compaction
    let session_config = SessionConfig {
        id: session.id.clone(),
        schedule_id: None,
        max_turns: None,
        retry_config: None,
    };

    let reply_stream = agent
        .reply(
            Message::user().with_text("Tell me more"),
            session_config,
            None,
        )
        .await?;
    tokio::pin!(reply_stream);

    // Track compaction and context size changes
    let mut compaction_occurred = false;
    let mut got_response = false;
    let mut input_tokens_after_compaction: Option<i32> = None;

    while let Some(event_result) = reply_stream.next().await {
        match event_result {
            Ok(AgentEvent::HistoryReplaced(_)) => {
                compaction_occurred = true;

                // Capture the input tokens immediately after compaction
                let session_after_compact = agent
                    .config
                    .session_manager
                    .get_session(&session.id, true)
                    .await?;
                input_tokens_after_compaction = session_after_compact.usage.input_tokens;
            }
            Ok(AgentEvent::Message(msg)) => {
                // Check if we got a real response (not just a notification)
                if msg
                    .content
                    .iter()
                    .any(|c| matches!(c, MessageContent::Text(_)))
                {
                    got_response = true;
                }
            }
            Ok(_) => {}
            Err(e) => return Err(e),
        }
    }

    // Verify recovery occurred
    assert!(
        compaction_occurred,
        "Compaction should have occurred due to context limit (>20000 tokens)"
    );
    assert!(
        got_response,
        "Should have received a response after recovery"
    );

    // Verify token counts
    let updated_session = agent
        .config
        .session_manager
        .get_session(&session.id, true)
        .await?;

    // Expected token flow:
    // 1. Initial attempt: >20000 tokens -> Context limit exceeded
    // 2. Compaction triggered:
    //    - Input: system prompt + messages (including long_tool_call with 15k tokens)
    //    - Output: 200 tokens (summary, as claimed by the mock)
    //    - New context size: estimated tokens of the retained conversation
    // 3. Retry with compacted context:
    //    - Input: system prompt + summary + new message
    //    - Output: 100 tokens (response)

    // Verify that current input context is dramatically reduced after compaction
    let tokens_after =
        input_tokens_after_compaction.expect("Should have captured tokens after compaction");

    // Before: system (6000) + long_tool_call messages (~15,400) = 21,400 (exceeded limit!)
    assert!(
        tokens_after > 0 && tokens_after < 200,
        "Input tokens after compaction should be the estimated retained context (under the mock's claimed 200). Got: {}",
        tokens_after
    );

    // The compacted context is now well under the 20k limit
    assert!(
        tokens_after < 20000,
        "Compacted context should be under 20k limit. Got: {}",
        tokens_after
    );

    // Check the final token state after recovery
    // Note: The session state reflects the RETRY call (after compaction),
    // which only sees agent-visible messages (summary + continuation + user message)
    let final_input = updated_session.usage.input_tokens.unwrap();
    let final_output = updated_session.usage.output_tokens;
    let final_total = updated_session.usage.total_tokens.unwrap();

    // After compaction, the retry only sees agent-visible messages:
    // Input: system (6000) + summary (~100) + continuation (~100) + user message (~100) = ~6300
    // Output: 200 (mock detects "summarized" in continuation as compaction)
    // Total: ~6500
    assert!(
        (6000..=6600).contains(&final_input),
        "Final input should reflect retry with agent-visible messages (~6300). Got: {}",
        final_input
    );

    assert_eq!(
        final_output,
        Some(200),
        "Final output should be 200 (mock detects continuation as compaction). Got: {:?}",
        final_output
    );

    assert_eq!(
        final_total,
        final_input + final_output.unwrap(),
        "Final total should equal input + output"
    );

    // Accumulated tokens should include all operations:
    // - Initial: 1000
    // - Compaction: ~6400 input (mock uses system_prompt.len()/4) + 200 output = ~6600
    // - Reply: ~6500 input + 200 output = ~6700
    // Total: 1000 + 6600 + 6700 = ~14300
    let accumulated = updated_session.accumulated_usage.total_tokens.unwrap();
    assert!(
        (13000..=16000).contains(&accumulated),
        "Accumulated should be ~14300 (initial + compaction + reply). Got: {}",
        accumulated
    );

    // Verify that the conversation was compacted
    let updated_conversation = updated_session
        .conversation
        .expect("Session should have conversation");
    assert_conversation_compacted(&updated_conversation);

    Ok(())
}

/// Drives a reply to completion, approving every tool the mock provider
/// requests, and returns (history replacements, final visible assistant text).
async fn run_reply_approving_tools(agent: &Agent, session: &Session) -> Result<(usize, String)> {
    let session_config = SessionConfig {
        id: session.id.clone(),
        schedule_id: None,
        max_turns: None,
        retry_config: None,
    };
    let reply_stream = agent
        .reply(
            Message::user().with_text("keep processing each result"),
            session_config,
            None,
        )
        .await?;
    tokio::pin!(reply_stream);

    let mut history_replacements = 0;
    let mut final_text = String::new();
    while let Some(event_result) = reply_stream.next().await {
        match event_result? {
            AgentEvent::HistoryReplaced(_) => history_replacements += 1,
            AgentEvent::Message(message) => {
                for content in &message.content {
                    if let MessageContent::ActionRequired(action) = content {
                        if let goose::conversation::message::ActionRequiredData::ToolConfirmation {
                            id,
                            ..
                        } = &action.data
                        {
                            agent
                                .submit_tool_confirmation(&session.id, id, Permission::AllowOnce)
                                .await?;
                        }
                    }
                }
                if let Some(text) = message.content.iter().find_map(|content| match content {
                    MessageContent::Text(text) if !text.text.is_empty() => Some(&text.text),
                    _ => None,
                }) {
                    final_text = text.clone();
                }
            }
            _ => {}
        }
    }
    Ok((history_replacements, final_text))
}

/// Runs a reply and returns the replacement count with every emitted
/// message rendered as text, system notifications included, so a test can
/// assert on what the client saw and in which order.
async fn run_reply_collecting_texts(
    agent: &Agent,
    session: &Session,
) -> Result<(usize, Vec<String>)> {
    let session_config = SessionConfig {
        id: session.id.clone(),
        schedule_id: None,
        max_turns: None,
        retry_config: None,
    };
    let reply_stream = agent
        .reply(
            Message::user().with_text("keep processing each result"),
            session_config,
            None,
        )
        .await?;
    tokio::pin!(reply_stream);

    let mut history_replacements = 0;
    let mut texts = Vec::new();
    while let Some(event_result) = reply_stream.next().await {
        match event_result? {
            AgentEvent::HistoryReplaced(_) => history_replacements += 1,
            AgentEvent::Message(message) => {
                texts.push(
                    message
                        .content
                        .iter()
                        .map(|content| content.to_string())
                        .collect::<String>(),
                );
            }
            _ => {}
        }
    }
    Ok((history_replacements, texts))
}

#[tokio::test]
async fn test_mid_turn_compaction_during_tool_loop() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let agent = Agent::new();

    // The session starts at 1,000 tokens against a 12,800-token trigger
    // (0.8 * the mock's declared 16k limit), so the turn-boundary check at
    // reply start passes. Each tool round-trip then inflates the context by
    // 15k tokens: after the first response lands, the session sits at ~21k
    // and the mid-turn re-check must compact before the next request hits
    // the mock's 20k streaming wall.
    let session = setup_test_session(&agent, &temp_dir, "mid-turn-compact-test", vec![]).await?;

    let provider = Arc::new(MockCompactionProvider::looping());
    agent
        .update_provider(
            provider.clone(),
            ModelConfig::new("mock-model"),
            &session.id,
        )
        .await?;

    let (history_replacements, final_text) = run_reply_approving_tools(&agent, &session).await?;

    assert_eq!(
        history_replacements, 1,
        "the tool loop should compact exactly once"
    );
    assert_eq!(final_text, "done after the tool loop");
    assert!(
        provider.has_compacted.load(Ordering::SeqCst),
        "the summarization call should have run"
    );
    assert_eq!(
        provider.context_limit_rejections(),
        0,
        "compaction must fire before the provider rejects an oversized request, not reactively after"
    );

    Ok(())
}

#[tokio::test]
async fn test_mid_turn_compaction_skipped_for_context_owning_provider() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let agent = Agent::new();

    let session =
        setup_test_session(&agent, &temp_dir, "mid-turn-own-context-test", vec![]).await?;

    // Session usage already past the 12,800-token trigger, so a mid-turn
    // check that ignored manages_own_context would compact during the loop.
    agent
        .config
        .session_manager
        .update(&session.id)
        .usage(Usage::new(Some(10_000), Some(10_000), Some(20_000)))
        .apply()
        .await?;

    let provider = Arc::new(MockCompactionProvider::context_owning_looping());
    agent
        .update_provider(
            provider.clone(),
            ModelConfig::new("mock-model"),
            &session.id,
        )
        .await?;

    let (history_replacements, final_text) = run_reply_approving_tools(&agent, &session).await?;

    assert_eq!(
        history_replacements, 0,
        "a context-owning provider never compacts"
    );
    assert_eq!(final_text, "done after the tool loop");
    assert!(
        !provider.has_compacted.load(Ordering::SeqCst),
        "no summarization call should have run"
    );
    assert_eq!(provider.context_limit_rejections(), 0);

    Ok(())
}

#[tokio::test]
async fn test_mid_turn_compaction_triggered_by_tool_output() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let agent = Agent::new();

    // Two `shell` calls land ~10k characters (≈5k tokens) of real truncated
    // output each, growing the conversation past the 8k-token threshold. The
    // lean input accounting keeps reported usage far below it: the turn was
    // under the threshold when its only provider call was made, so only a
    // mid-turn check that recounts the conversation can see the growth.
    let session =
        setup_test_session(&agent, &temp_dir, "mid-turn-tool-output-test", vec![]).await?;
    add_shell_extension(&agent, &session, &temp_dir).await?;

    let provider = Arc::new(MockCompactionProvider::result_looping());
    agent
        .update_provider(
            provider.clone(),
            ModelConfig::new("mock-model"),
            &session.id,
        )
        .await?;

    let (history_replacements, final_text) = run_reply_approving_tools(&agent, &session).await?;

    assert_eq!(
        history_replacements, 1,
        "the tool responses alone must cross the threshold mid-turn"
    );
    assert_eq!(final_text, "done after the tool loop");
    assert!(
        provider.has_compacted.load(Ordering::SeqCst),
        "the summarization call should have run"
    );
    assert_eq!(provider.context_limit_rejections(), 0);

    Ok(())
}

#[tokio::test]
async fn test_mid_turn_compaction_skipped_when_final_output_is_ready() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let agent = Agent::new();

    // The thinking padding drives reported usage past the 12.8k-token
    // threshold in the same round-trip that collects the recipe result, so a
    // mid-turn check that ignored the completed recipe would summarize
    // between the response and its delivery.
    let session =
        setup_test_session(&agent, &temp_dir, "mid-turn-final-output-test", vec![]).await?;
    agent
        .apply_recipe_components(
            Some(Response {
                json_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": { "result": { "type": "string" } },
                    "required": ["result"]
                })),
            }),
            true,
        )
        .await?;

    let provider = Arc::new(MockCompactionProvider::final_output());
    agent
        .update_provider(
            provider.clone(),
            ModelConfig::new("mock-model"),
            &session.id,
        )
        .await?;

    let (history_replacements, final_text) = run_reply_approving_tools(&agent, &session).await?;

    assert_eq!(
        history_replacements, 0,
        "the collected final output must be delivered, not summarized first"
    );
    assert_eq!(final_text, r#"{"result":"42"}"#);
    assert!(
        !provider.has_compacted.load(Ordering::SeqCst),
        "no summarization call should have run"
    );
    assert_eq!(provider.context_limit_rejections(), 0);

    Ok(())
}

/// Adds the developer `shell` extension, whose real truncated output grows
/// the conversation mid-turn.
async fn add_shell_extension(agent: &Agent, session: &Session, temp_dir: &TempDir) -> Result<()> {
    agent
        .extension_manager
        .add_extension(
            ExtensionConfig::Platform {
                name: "developer".to_string(),
                description: "developer tools".to_string(),
                display_name: None,
                bundled: None,
                available_tools: vec!["shell".to_string()],
            },
            Some(temp_dir.path().to_path_buf()),
            None,
            Some(&session.id),
        )
        .await
        .map_err(anyhow::Error::from)
}

#[tokio::test]
async fn test_mid_turn_compaction_adds_tool_growth_to_the_provider_baseline() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let agent = Agent::new();

    // A single `shell` call lands ~5k tokens of real output. The recounted
    // conversation (~5.5k) and the reported usage (~6.5k, dominated by the 6k
    // system overhead) each stay under the 8k threshold, so trusting only the
    // larger of the two misses the crossing; the unsent tool growth must be
    // added to the provider's baseline for the check to see it.
    let session = setup_test_session(&agent, &temp_dir, "mid-turn-baseline-test", vec![]).await?;
    add_shell_extension(&agent, &session, &temp_dir).await?;

    let provider = Arc::new(MockCompactionProvider::baseline_growth());
    agent
        .update_provider(
            provider.clone(),
            ModelConfig::new("mock-model"),
            &session.id,
        )
        .await?;

    let (history_replacements, final_text) = run_reply_approving_tools(&agent, &session).await?;

    assert_eq!(
        history_replacements, 1,
        "the unsent tool growth must be added to the reported usage baseline"
    );
    assert_eq!(final_text, "done after the tool loop");
    assert!(
        provider.has_compacted.load(Ordering::SeqCst),
        "the summarization call should have run"
    );
    assert_eq!(provider.context_limit_rejections(), 0);

    Ok(())
}

#[tokio::test]
async fn test_mid_turn_compaction_counts_every_parallel_result() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let agent = Agent::new();

    // Three parallel `shell` calls each land ~1k tokens of real output. The
    // reported baseline (~6.2k) plus any single result, and the recounted
    // conversation alone (~3.5k), each stay under the 8k threshold — only
    // the baseline plus the whole batch crosses. The legacy loop persists
    // the batch as alternating request/response assistant/user pairs, so the
    // recount must treat the usage-carrying first split as the boundary
    // rather than the newest assistant message, which is a synthetic split
    // of the same response and would hide the earlier results' growth.
    let session =
        setup_test_session(&agent, &temp_dir, "mid-turn-parallel-batch-test", vec![]).await?;
    add_shell_extension(&agent, &session, &temp_dir).await?;

    let provider = Arc::new(MockCompactionProvider::parallel_baseline_growth());
    agent
        .update_provider(
            provider.clone(),
            ModelConfig::new("mock-model"),
            &session.id,
        )
        .await?;

    let (history_replacements, final_text) = run_reply_approving_tools(&agent, &session).await?;

    assert_eq!(
        history_replacements, 1,
        "every parallel result must count as unsent growth on the baseline"
    );
    assert_eq!(final_text, "done after the tool loop");
    assert!(
        provider.has_compacted.load(Ordering::SeqCst),
        "the summarization call should have run"
    );
    assert_eq!(provider.context_limit_rejections(), 0);

    Ok(())
}

#[tokio::test]
async fn test_mid_turn_compaction_skipped_when_cancelled_during_tools() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let agent = Agent::new();

    // The reply padding drives the session past the threshold in the same
    // round-trip whose `shell` call sleeps, so the mid-turn check is reached
    // over-threshold while the tool is still executing — the window a client
    // cancellation lands in. A cancelled turn must not start a summarization
    // request; the state machine suppresses every operation once cancelled.
    let session = setup_test_session(&agent, &temp_dir, "mid-turn-cancel-test", vec![]).await?;
    add_shell_extension(&agent, &session, &temp_dir).await?;

    let provider = Arc::new(MockCompactionProvider::cancellable());
    agent
        .update_provider(
            provider.clone(),
            ModelConfig::new("mock-model"),
            &session.id,
        )
        .await?;

    let session_config = SessionConfig {
        id: session.id.clone(),
        schedule_id: None,
        max_turns: None,
        retry_config: None,
    };
    let cancel_token = CancellationToken::new();
    let reply_stream = agent
        .reply(
            Message::user().with_text("keep processing each result"),
            session_config,
            Some(cancel_token.clone()),
        )
        .await?;
    tokio::pin!(reply_stream);

    let mut history_replacements = 0;
    while let Some(event_result) = reply_stream.next().await {
        match event_result? {
            AgentEvent::HistoryReplaced(_) => history_replacements += 1,
            AgentEvent::Message(message)
                if message
                    .content
                    .iter()
                    .any(|content| matches!(content, MessageContent::ToolRequest(_))) =>
            {
                // The tool-request message is yielded before dispatch: the
                // shell sleeps for a second, so cancelling here lands while
                // the tool is still executing.
                cancel_token.cancel();
            }
            _ => {}
        }
    }

    assert_eq!(
        history_replacements, 0,
        "a cancelled turn must not start a compaction request"
    );
    assert!(
        !provider.has_compacted.load(Ordering::SeqCst),
        "no summarization call should have run"
    );

    Ok(())
}

#[tokio::test]
async fn test_mid_turn_compaction_failure_continues_the_turn() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let agent = Agent::new();

    // The two `shell` outputs cross the 8k threshold, but every summarization
    // request fails. The turn must continue to the next inference instead of
    // ending: the request that follows may still fit, and the reactive path
    // owns the outcome if it does not.
    let session = setup_test_session(&agent, &temp_dir, "mid-turn-failure-test", vec![]).await?;
    add_shell_extension(&agent, &session, &temp_dir).await?;

    let provider = Arc::new(MockCompactionProvider::failing_summarization());
    agent
        .update_provider(
            provider.clone(),
            ModelConfig::new("mock-model"),
            &session.id,
        )
        .await?;

    let (history_replacements, final_text) = run_reply_approving_tools(&agent, &session).await?;

    assert_eq!(
        history_replacements, 0,
        "the failed compaction must not replace the history"
    );
    assert_eq!(final_text, "done after the tool loop");
    assert!(
        provider.has_compacted.load(Ordering::SeqCst),
        "the summarization attempt should have run"
    );
    assert_eq!(provider.context_limit_rejections(), 0);

    Ok(())
}

#[tokio::test]
async fn test_failed_compaction_still_announces_the_auto_compaction() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let agent = Agent::new();

    // The turn-boundary compaction fails. The announcement and progress
    // notice must still reach the client, because they are yielded before
    // the summarization request starts: a client watching a slow request
    // needs the progress indication, and a failed one must not swallow the
    // events entirely — the state machine emits its notifications before
    // awaiting the provider for the same reason.
    let messages = vec![
        Message::user().with_text("x ".repeat(10_000)),
        Message::assistant().with_text("done"),
    ];
    let session =
        setup_test_session(&agent, &temp_dir, "announce-on-failure-test", messages).await?;

    let provider = Arc::new(MockCompactionProvider {
        fail_summarizations: true,
        ..MockCompactionProvider::conversational()
    });
    agent
        .update_provider(
            provider.clone(),
            ModelConfig::new("mock-model"),
            &session.id,
        )
        .await?;

    let (history_replacements, texts) = run_reply_collecting_texts(&agent, &session).await?;

    assert_eq!(
        history_replacements, 0,
        "the failed compaction must not replace the history"
    );
    assert!(
        provider.has_compacted.load(Ordering::SeqCst),
        "the summarization attempt should have run"
    );
    let announcement = texts
        .iter()
        .position(|text| text.contains("Performing auto-compaction"))
        .expect("the failed compaction must still announce itself");
    let progress = texts
        .iter()
        .position(|text| text.contains("goose is compacting the conversation"))
        .expect("the failed compaction must still show its progress notice");
    let error = texts
        .iter()
        .position(|text| text.contains("Ran into this error trying to compact"))
        .expect("the failed compaction must still report its error");
    assert!(
        announcement < error && progress < error,
        "the notices must precede the error report"
    );

    Ok(())
}

#[tokio::test]
async fn test_auto_compaction_floors_on_the_recounted_conversation() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let agent = Agent::new();

    // The session metadata claims 1,000 tokens while the persisted history
    // holds far more against the 8k threshold, and the bulk of it sits before
    // the newest assistant message. Only a recount whose total covers the
    // whole conversation can floor the stale metadata; one that stopped at
    // the newest assistant message would send the oversized history on.
    let messages = vec![
        Message::user().with_text("x ".repeat(10_000)),
        Message::assistant().with_text("done"),
    ];
    let session =
        setup_test_session(&agent, &temp_dir, "auto-compact-floor-test", messages).await?;

    let provider = Arc::new(MockCompactionProvider::conversational());
    agent
        .update_provider(
            provider.clone(),
            ModelConfig::new("mock-model"),
            &session.id,
        )
        .await?;

    let (history_replacements, final_text) = run_reply_approving_tools(&agent, &session).await?;

    assert_eq!(
        history_replacements, 1,
        "the recounted conversation must floor the stale metadata"
    );
    assert_eq!(final_text, "done after the tool loop");
    assert!(
        provider.has_compacted.load(Ordering::SeqCst),
        "the summarization call should have run"
    );
    assert_eq!(provider.context_limit_rejections(), 0);

    Ok(())
}

#[tokio::test]
async fn test_empty_response_usage_preserves_the_recount_boundary() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let agent = Agent::new();

    // A large kickoff (~2k tokens) is answered by an empty response that
    // still reports usage (~6.2k against the 8k threshold), and the loop
    // drops the empty assistant message. A steer queued during that
    // round-trip (~300 tokens) revives the turn, and the steer re-check
    // recounts: the session baseline already contains the kickoff, so only
    // the steer may count as growth — 6.2k + 0.3k stays under the threshold
    // and no compaction may run. A recount that lost the dropped response's
    // boundary would fall back to the previous marker, sweep the
    // already-sent kickoff back in as unsent growth (~8.5k), and compact a
    // context whose real next request still fits.
    let session =
        setup_test_session(&agent, &temp_dir, "empty-reply-boundary-test", vec![]).await?;

    let provider = Arc::new(MockCompactionProvider::empty_then_conversational());
    agent
        .update_provider(
            provider.clone(),
            ModelConfig::new("mock-model"),
            &session.id,
        )
        .await?;

    let session_config = SessionConfig {
        id: session.id.clone(),
        schedule_id: None,
        max_turns: None,
        retry_config: None,
    };
    let reply_stream = agent
        .reply(
            Message::user().with_text("x ".repeat(2_000)),
            session_config,
            None,
        )
        .await?;
    tokio::pin!(reply_stream);

    let mut history_replacements = 0;
    let mut final_text = String::new();
    let mut steered = false;
    while let Some(event_result) = reply_stream.next().await {
        match event_result? {
            AgentEvent::HistoryReplaced(_) => history_replacements += 1,
            AgentEvent::Usage(_) => {
                // The empty response yields no message events, so the usage
                // event is the suspension point: queueing here guarantees
                // the steer is pending when the loop decides whether the
                // empty turn continues.
                if !steered {
                    steered = true;
                    agent
                        .steer(
                            &session.id,
                            Message::user()
                                .with_text(format!("steered onward {}", "x ".repeat(300))),
                        )
                        .await;
                }
            }
            AgentEvent::Message(message) => {
                if let Some(text) = message.content.iter().find_map(|content| match content {
                    MessageContent::Text(text) if !text.text.is_empty() => Some(&text.text),
                    _ => None,
                }) {
                    final_text = text.clone();
                }
            }
            _ => {}
        }
    }

    assert!(
        steered,
        "the steer must be queued while the empty response is processed"
    );
    assert_eq!(
        history_replacements, 0,
        "the already-sent kickoff must not be recounted as unsent growth"
    );
    assert!(
        !provider.has_compacted.load(Ordering::SeqCst),
        "no summarization may run while the real next request still fits"
    );
    assert_eq!(final_text, "This is a mock response.");
    assert_eq!(provider.context_limit_rejections(), 0);

    Ok(())
}

#[tokio::test]
async fn test_steer_after_a_text_only_reply_rechecks_the_threshold() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let agent = Agent::new();

    // The kickoff gets a text-only reply (~6.2k reported tokens), so no tools
    // were called and the mid-turn check never runs. The steer that revives
    // the turn is large enough to push past the 8k threshold, so the turn
    // must be re-checked once the steer is drained, before the request
    // carries it to the provider.
    let session = setup_test_session(&agent, &temp_dir, "steer-recheck-test", vec![]).await?;

    let provider = Arc::new(MockCompactionProvider::conversational());
    agent
        .update_provider(
            provider.clone(),
            ModelConfig::new("mock-model"),
            &session.id,
        )
        .await?;

    let session_config = SessionConfig {
        id: session.id.clone(),
        schedule_id: None,
        max_turns: None,
        retry_config: None,
    };
    let reply_stream = agent
        .reply(
            Message::user().with_text("answer without tools"),
            session_config,
            None,
        )
        .await?;
    tokio::pin!(reply_stream);

    let mut history_replacements = 0;
    let mut final_text = String::new();
    let mut steered = false;
    while let Some(event_result) = reply_stream.next().await {
        match event_result? {
            AgentEvent::HistoryReplaced(_) => history_replacements += 1,
            AgentEvent::Message(message) => {
                // The stream suspends at this yield before the turn-end
                // handling runs, so queueing here guarantees the steer is
                // pending when the loop decides whether the turn continues.
                if !steered
                    && message.role == rmcp::model::Role::Assistant
                    && message
                        .content
                        .iter()
                        .any(|content| matches!(content, MessageContent::Text(text) if !text.text.is_empty()))
                {
                    steered = true;
                    agent
                        .steer(&session.id, Message::user().with_text("x ".repeat(6_000)))
                        .await;
                }
                if let Some(text) = message.content.iter().find_map(|content| match content {
                    MessageContent::Text(text) if !text.text.is_empty() => Some(&text.text),
                    _ => None,
                }) {
                    final_text = text.clone();
                }
            }
            _ => {}
        }
    }

    assert!(
        steered,
        "the steer must be queued after the text-only reply lands"
    );
    assert_eq!(
        history_replacements, 1,
        "the drained steer must be re-checked against the threshold"
    );
    assert_eq!(final_text, "done after the tool loop");
    assert!(
        provider.has_compacted.load(Ordering::SeqCst),
        "the summarization call should have run"
    );
    assert_eq!(provider.context_limit_rejections(), 0);

    Ok(())
}

#[tokio::test]
async fn goal_nudge_after_a_text_only_reply_rechecks_the_threshold() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let agent = Agent::new();

    // The kickoff gets a text-only reply, so the mid-turn check never runs.
    // The goal nudge the loop then appends is agent-only, so no user turn
    // boundary follows it either — without the pre-inference re-check the
    // request that carries the nudge would leave over the threshold.
    let session = setup_test_session(&agent, &temp_dir, "goal-nudge-recheck-test", vec![]).await?;

    let provider = Arc::new(MockCompactionProvider::conversational());
    agent
        .update_provider(
            provider.clone(),
            ModelConfig::new("mock-model"),
            &session.id,
        )
        .await?;

    agent.set_goal(Some("x ".repeat(6_000))).await;

    let session_config = SessionConfig {
        id: session.id.clone(),
        schedule_id: None,
        max_turns: None,
        retry_config: None,
    };
    let reply_stream = agent
        .reply(
            Message::user().with_text("answer without tools"),
            session_config,
            None,
        )
        .await?;
    tokio::pin!(reply_stream);

    let mut history_replacements = 0;
    let mut final_text = String::new();
    while let Some(event_result) = reply_stream.next().await {
        match event_result? {
            AgentEvent::HistoryReplaced(_) => history_replacements += 1,
            AgentEvent::Message(message) => {
                if let Some(text) = message.content.iter().find_map(|content| match content {
                    MessageContent::Text(text) if !text.text.is_empty() => Some(&text.text),
                    _ => None,
                }) {
                    final_text = text.clone();
                }
            }
            _ => {}
        }
    }

    assert_eq!(
        history_replacements, 1,
        "the appended goal nudge must be re-checked against the threshold"
    );
    assert_eq!(final_text, "done after the tool loop");
    assert!(
        provider.has_compacted.load(Ordering::SeqCst),
        "the summarization call should have run"
    );
    assert_eq!(provider.context_limit_rejections(), 0);
    assert!(
        agent.get_goal().await.is_none(),
        "the nudge cycle should have cleared the goal before ending the turn"
    );

    Ok(())
}
