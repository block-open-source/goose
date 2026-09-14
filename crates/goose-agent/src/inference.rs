//! Provider inference operation for the unrolled agent loop.

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use futures::StreamExt;
use goose_provider_types::base::Provider;
use goose_provider_types::conversation::message::{InferenceMetadata, Message, MessageContent};
use goose_provider_types::conversation::token_usage::ProviderUsage;
use goose_provider_types::conversation::{
    effective_role, fix_conversation, merge_consecutive_messages_for_request, Conversation,
    EffectiveRole,
};
use goose_provider_types::errors::ProviderError;
use goose_provider_types::model::ModelConfig;
use tracing_futures::Instrument;

use crate::operation::{
    applied, messages_since_kickoff, not_applicable, trailing_error, yielded_with, Emitter,
    Inference, InferenceInput, Operation, OperationResult,
};

pub struct PreparedInferenceRequest {
    pub system_prompt: String,
    pub tools: Vec<rmcp::model::Tool>,
    pub additional_messages: Vec<Message>,
}

#[async_trait]
pub trait InferenceRequestPreparer<S>: Send + Sync {
    async fn prepare(
        &self,
        session: &S,
        conversation: &Conversation,
        input: InferenceInput,
    ) -> Result<PreparedInferenceRequest>;
}

pub struct IdentityInferenceRequestPreparer;

#[async_trait]
impl<S: Sync> InferenceRequestPreparer<S> for IdentityInferenceRequestPreparer {
    async fn prepare(
        &self,
        _session: &S,
        _conversation: &Conversation,
        input: InferenceInput,
    ) -> Result<PreparedInferenceRequest> {
        Ok(PreparedInferenceRequest {
            system_prompt: input
                .prompt_parts
                .into_iter()
                .map(|(_, part)| part)
                .collect::<Vec<_>>()
                .join("\n\n"),
            tools: input.tools,
            additional_messages: Vec::new(),
        })
    }
}

pub trait InferenceEffect: From<Message> + Send + 'static {
    fn record_usage(usage: ProviderUsage) -> Self;
}

const EMPTY_RESPONSE_MESSAGE: &str =
    "The model returned an empty response. Please resend your message to continue.";
const CANCELLED_TOOL_RESPONSE: &str = "Tool call was cancelled before execution";

fn is_thinking(content: &MessageContent) -> bool {
    matches!(
        content,
        MessageContent::Thinking(_) | MessageContent::RedactedThinking(_)
    )
}

fn normalize_tool_call_thinking(accumulator: &mut Conversation, chunk: &mut Message) {
    if !chunk
        .content
        .iter()
        .any(|content| matches!(content, MessageContent::ToolRequest(_)))
    {
        return;
    }

    let has_direct_thinking = chunk.content.iter().any(is_thinking);
    let mut prior_thinking = Vec::new();
    for message in accumulator.messages_mut() {
        if message.role != chunk.role
            || message
                .content
                .iter()
                .any(|content| matches!(content, MessageContent::ToolRequest(_)))
        {
            continue;
        }
        prior_thinking.extend(
            message
                .content
                .iter()
                .filter(|content| is_thinking(content))
                .cloned(),
        );
        message.content.retain(|content| !is_thinking(content));
    }
    accumulator
        .messages_mut()
        .retain(|message| !message.content.is_empty());

    if !has_direct_thinking && !prior_thinking.is_empty() {
        if let Some(tool_request) = chunk
            .content
            .iter()
            .position(|content| matches!(content, MessageContent::ToolRequest(_)))
        {
            chunk
                .content
                .splice(tool_request..tool_request, prior_thinking);
        }
    }
}

pub fn chat_span(
    provider: &dyn Provider,
    model_config: &ModelConfig,
    session_id: &str,
    purpose: &'static str,
) -> tracing::Span {
    let span = tracing::info_span!(
        target: "goose::state_machine",
        "chat",
        "gen_ai.operation.name" = "chat",
        "gen_ai.provider.name" = %provider.get_name(),
        "gen_ai.request.model" = %model_config.model_name,
        "gen_ai.request.temperature" = tracing::field::Empty,
        "gen_ai.request.max_tokens" = tracing::field::Empty,
        "gen_ai.response.model" = tracing::field::Empty,
        "gen_ai.response.finish_reasons" = tracing::field::Empty,
        "gen_ai.response.id" = tracing::field::Empty,
        "gen_ai.usage.input_tokens" = tracing::field::Empty,
        "gen_ai.usage.output_tokens" = tracing::field::Empty,
        "goose.chat.purpose" = purpose,
        "error.type" = tracing::field::Empty,
        session.id = %session_id,
    );
    record_request_params(&span, model_config);
    span
}

fn is_empty_response(message: &Message) -> bool {
    message.content.iter().all(|content| match content {
        MessageContent::Text(text) => text.text.trim().is_empty(),
        MessageContent::Thinking(thinking) => {
            thinking.thinking.trim().is_empty() && thinking.signature.is_empty()
        }
        _ => false,
    })
}

fn record_request_params(span: &tracing::Span, model_config: &ModelConfig) {
    if let Some(temperature) = model_config.temperature {
        span.record("gen_ai.request.temperature", temperature as f64);
    }
    if let Some(max_tokens) = model_config.max_tokens {
        span.record("gen_ai.request.max_tokens", max_tokens as i64);
    }
}

pub fn record_chat_usage(span: &tracing::Span, usage: &ProviderUsage) {
    span.record("gen_ai.response.model", usage.model.as_str());
    if let Some(tokens) = usage.usage.input_tokens {
        span.record("gen_ai.usage.input_tokens", tokens);
    }
    if let Some(tokens) = usage.usage.output_tokens {
        span.record("gen_ai.usage.output_tokens", tokens);
    }
    if let Some(tokens) = usage.usage.cache_read_input_tokens {
        span.record("gen_ai.usage.cache_read.input_tokens", tokens);
    }
    if let Some(tokens) = usage.usage.cache_write_input_tokens {
        span.record("gen_ai.usage.cache_creation.input_tokens", tokens);
    }
    if let Some(reasons) = &usage.finish_reasons {
        let reasons_json = serde_json::to_string(reasons).unwrap_or_default();
        span.record("gen_ai.response.finish_reasons", reasons_json.as_str());
    }
    if let Some(id) = &usage.response_id {
        span.record("gen_ai.response.id", id.as_str());
    }
}

pub struct InferenceRunner<'a, S, E> {
    provider: Arc<dyn Provider>,
    model_config: ModelConfig,
    request_preparer: Arc<dyn InferenceRequestPreparer<S> + 'a>,
    effect: std::marker::PhantomData<fn() -> E>,
}

/// The agent-visible conversation as the provider sees it: tool requests left
/// unanswered by an earlier turn are dropped, since nothing will answer them now.
fn messages_for_provider(conversation: &Conversation, turn: &[Message]) -> Vec<Message> {
    let answered: std::collections::HashSet<&str> = conversation
        .messages()
        .iter()
        .flat_map(|message| message.get_tool_response_ids())
        .collect();
    let start = conversation.len() - turn.len();
    conversation
        .messages()
        .iter()
        .enumerate()
        .filter(|(_, message)| message.is_agent_visible())
        .map(|(index, message)| {
            let mut message = message.agent_visible_content();
            if index < start {
                message.content.retain(|content| match content {
                    MessageContent::ToolRequest(request) => answered.contains(request.id.as_str()),
                    _ => true,
                });
            }
            message
        })
        .filter(|message| !message.content.is_empty())
        .collect()
}

fn latest_provider_session_id<'a>(
    conversation: &'a Conversation,
    provider: &str,
) -> Option<&'a str> {
    conversation
        .messages()
        .iter()
        .rev()
        .find_map(|message| message.metadata.inference.as_ref())
        .filter(|inference| inference.provider == provider)
        .and_then(|inference| inference.provider_session_id.as_deref())
}

fn ends_with_provider_turn(messages: &[Message]) -> bool {
    messages.last().is_some_and(|message| {
        matches!(
            effective_role(message),
            EffectiveRole::User | EffectiveRole::Tool
        )
    })
}

fn cancellation_response(persisted: &[Message], pending: &[Message]) -> Option<Message> {
    let mut answered = persisted
        .iter()
        .chain(pending)
        .flat_map(Message::get_tool_response_ids)
        .collect::<std::collections::HashSet<_>>();
    let mut request_ids = std::collections::HashSet::new();
    let mut response = Message::user();
    for request in persisted
        .iter()
        .chain(pending)
        .flat_map(|message| &message.content)
        .filter_map(MessageContent::as_tool_request)
    {
        if request_ids.insert(request.id.as_str()) && !answered.remove(request.id.as_str()) {
            response.add_tool_response_with_metadata(
                request.id.clone(),
                Ok(rmcp::model::CallToolResult::error(vec![
                    rmcp::model::ContentBlock::text(CANCELLED_TOOL_RESPONSE),
                ])),
                request.metadata.as_ref(),
            );
        }
    }
    (!response.get_tool_response_ids().is_empty()).then_some(response)
}

fn inference_span(provider: &dyn Provider, model_config: &ModelConfig) -> tracing::Span {
    let span = tracing::info_span!(
        target: "goose::state_machine",
        "chat",
        "gen_ai.operation.name" = "chat",
        "gen_ai.provider.name" = %provider.get_name(),
        "gen_ai.request.model" = %model_config.model_name,
        "gen_ai.request.temperature" = tracing::field::Empty,
        "gen_ai.request.max_tokens" = tracing::field::Empty,
        "gen_ai.response.model" = tracing::field::Empty,
        "gen_ai.response.finish_reasons" = tracing::field::Empty,
        "gen_ai.response.id" = tracing::field::Empty,
        "gen_ai.usage.input_tokens" = tracing::field::Empty,
        "gen_ai.usage.output_tokens" = tracing::field::Empty,
        "error.type" = tracing::field::Empty,
    );
    record_request_params(&span, model_config);
    span
}

impl<'a, S: Sync, E: InferenceEffect> InferenceRunner<'a, S, E> {
    pub fn new(provider: Arc<dyn Provider>, model_config: ModelConfig) -> Self {
        Self {
            provider,
            model_config,
            request_preparer: Arc::new(IdentityInferenceRequestPreparer),
            effect: std::marker::PhantomData,
        }
    }

    pub fn with_request_preparer(
        mut self,
        request_preparer: Arc<dyn InferenceRequestPreparer<S> + 'a>,
    ) -> Self {
        self.request_preparer = request_preparer;
        self
    }

    async fn error_outcome(&self, err: &ProviderError, emit: &Emitter) -> Vec<E> {
        tracing::Span::current().record("error.type", err.telemetry_type());
        tracing::error!("LLM provider error: {err}");
        let message = Message::from_provider_error(err);
        let message = emit.message(message).await;
        vec![E::from(message)]
    }
}

#[async_trait]
impl<S: Sync, E: InferenceEffect> Operation<S, E> for InferenceRunner<'_, S, E> {
    fn name(&self) -> &'static str {
        "llm"
    }

    async fn cancel(
        &self,
        _session: &S,
        conversation: &Conversation,
        result: OperationResult<E>,
        emit: &Emitter,
    ) -> Result<OperationResult<E>> {
        let OperationResult::NotApplicable = result else {
            return Ok(result);
        };
        let Some(response) = cancellation_response(messages_since_kickoff(conversation)?, &[])
        else {
            return Ok(OperationResult::NotApplicable);
        };
        let response = emit.message(response).await;
        applied([E::from(response)])
    }
}

#[async_trait]
impl<S: Sync, E: InferenceEffect> Inference<S, E> for InferenceRunner<'_, S, E> {
    fn applies(&self, conversation: &Conversation) -> bool {
        let Ok(turn) = messages_since_kickoff(conversation) else {
            return false;
        };
        trailing_error(conversation).is_none()
            && ends_with_provider_turn(&messages_for_provider(conversation, turn))
    }

    async fn infer(
        &self,
        session: &S,
        conversation: &Conversation,
        input: InferenceInput,
        emit: &Emitter,
    ) -> Result<OperationResult<E>> {
        let messages = messages_since_kickoff(conversation)?;
        if trailing_error(conversation).is_some() {
            return not_applicable();
        }

        let mut messages_for_provider = messages_for_provider(conversation, messages);
        if !ends_with_provider_turn(&messages_for_provider) {
            return not_applicable();
        }

        let span = inference_span(self.provider.as_ref(), &self.model_config);

        async {
            let PreparedInferenceRequest {
                system_prompt,
                tools,
                additional_messages,
            } = self
                .request_preparer
                .prepare(session, conversation, input)
                .await?;

            for message in &additional_messages {
                messages_for_provider.push(message.clone());
            }
            let mut usage_effects: Vec<E> = additional_messages.into_iter().map(E::from).collect();

            let provider_name = self.provider.get_name();
            if let Some(session_id) = latest_provider_session_id(conversation, provider_name) {
                if let Err(error) = self.provider.resume(session_id).await {
                    tracing::warn!(
                        provider = provider_name,
                        %error,
                        "Could not resume provider session; continuing with a handoff"
                    );
                }
            }

            let projected =
                Conversation::new_unvalidated(messages_for_provider).agent_visible_messages();
            let (fixed, _) = fix_conversation(Conversation::new_unvalidated(projected));
            let conversation_for_provider = Conversation::new_unvalidated(
                merge_consecutive_messages_for_request(fixed.messages().clone()),
            );
            let stream = self
                .provider
                .stream(
                    &self.model_config,
                    &system_prompt,
                    conversation_for_provider.messages(),
                    &tools,
                )
                .await;

            let mut stream = match stream {
                Ok(stream) => stream,
                Err(err) => {
                    usage_effects.extend(self.error_outcome(&err, emit).await);
                    return applied(usage_effects);
                }
            };

            let requested_model = self.model_config.model_name.clone();
            let resolved_model = self
                .provider
                .fetch_model_info(&requested_model)
                .await
                .ok()
                .and_then(|model_info| model_info.resolved_model);
            let provider_session_id = self.provider.provider_session_id();
            let inference = Some(InferenceMetadata {
                provider: self.provider.get_name().to_string(),
                requested_model,
                resolved_model,
                provider_session_id,
            });

            let mut accumulator = Conversation::empty();
            let mut tool_request_ids = std::collections::HashSet::new();
            let mut provider_usage = None;
            let mut cancelled = false;
            loop {
                tokio::select! {
                    biased;
                    _ = emit.cancelled() => {
                        cancelled = true;
                        break;
                    },
                    next = stream.next() => {
                        let Some(result) = next else { break };
                        let (msg_opt, usage_opt) = match result {
                            Ok(chunk) => chunk,
                            Err(err) => {
                                if let Some(usage) = provider_usage {
                                    usage_effects.push(E::record_usage(usage));
                                }
                                usage_effects.extend(accumulator.into_iter().map(E::from));
                                usage_effects.extend(self.error_outcome(&err, emit).await);
                                return applied(usage_effects);
                            }
                        };
                        if let Some(usage) = usage_opt {
                            let span = tracing::Span::current();
                            record_chat_usage(&span, &usage);
                            provider_usage = Some(usage);
                        }
                        if let Some(mut chunk) = msg_opt {
                            if let Some(inference) = &inference {
                                chunk = chunk.with_inference_if_assistant(inference.clone());
                            }
                            chunk.content.retain(|content| match content {
                                MessageContent::ToolRequest(request) => {
                                    tool_request_ids.insert(request.id.clone())
                                }
                                _ => true,
                            });
                            normalize_tool_call_thinking(&mut accumulator, &mut chunk);
                            if chunk.content.is_empty() {
                                if chunk.metadata.output_token_limit_reached {
                                    chunk = emit.message(chunk).await;
                                }
                                accumulator.push(chunk);
                                continue;
                            }
                            let chunk = emit.message(chunk).await;
                            accumulator.push(chunk);
                        }
                    }
                }
            }

            if let Some(usage) = provider_usage {
                usage_effects.push(E::record_usage(usage));
            }

            if cancelled || emit.cancel_token().is_cancelled() {
                if let Some(response) = cancellation_response(messages, accumulator.messages()) {
                    let response = emit.message(response).await;
                    accumulator.push(response);
                }
            }

            let empty_response = !cancelled
                && !accumulator
                    .iter()
                    .any(|message| message.metadata.output_token_limit_reached)
                && accumulator.iter().all(is_empty_response);
            if empty_response {
                let message = Message::assistant().with_text(EMPTY_RESPONSE_MESSAGE);
                let message = emit.message(message).await;
                usage_effects.push(E::from(message));
                return yielded_with(usage_effects);
            }

            usage_effects.extend(accumulator.into_iter().map(|message| E::from(message)));
            applied(usage_effects)
        }
        .instrument(span)
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_session_id_comes_only_from_latest_inference() {
        let conversation = Conversation::new_unvalidated([
            Message::assistant().with_inference(InferenceMetadata {
                provider: "provider-a".to_string(),
                requested_model: "model".to_string(),
                resolved_model: None,
                provider_session_id: Some("session-a".to_string()),
            }),
            Message::assistant().with_inference(InferenceMetadata {
                provider: "provider-b".to_string(),
                requested_model: "model".to_string(),
                resolved_model: None,
                provider_session_id: Some("session-b".to_string()),
            }),
        ]);

        assert_eq!(
            latest_provider_session_id(&conversation, "provider-b"),
            Some("session-b")
        );
        assert_eq!(
            latest_provider_session_id(&conversation, "provider-a"),
            None
        );
    }

    #[test]
    fn cancellation_response_includes_requests_from_unconverted_messages() {
        let persisted = [Message::user().with_text("run it")];
        let pending = [Message::assistant().with_tool_request(
            "pending-call",
            Ok(rmcp::model::CallToolRequestParams::new("tool")),
        )];

        let response = cancellation_response(&persisted, &pending).expect("cancellation response");

        assert_eq!(
            response.get_tool_response_ids(),
            std::collections::HashSet::from(["pending-call"])
        );
        let cancellation_text = response
            .content
            .iter()
            .filter_map(MessageContent::as_tool_response)
            .flat_map(|response| {
                response
                    .tool_result
                    .as_ref()
                    .expect("tool result")
                    .content
                    .iter()
            })
            .filter_map(|content| content.as_text())
            .map(|text| text.text.as_str())
            .collect::<Vec<_>>();
        assert_eq!(cancellation_text, vec![CANCELLED_TOOL_RESPONSE]);
    }

    #[test]
    fn cancellation_response_skips_answered_requests() {
        let request = Message::assistant().with_tool_request(
            "answered-call",
            Ok(rmcp::model::CallToolRequestParams::new("tool")),
        );
        let response = Message::user().with_tool_response(
            "answered-call",
            Ok(rmcp::model::CallToolResult::success(vec![])),
        );

        assert!(cancellation_response(&[request, response], &[]).is_none());
    }

    #[test]
    fn signed_thinking_without_text_is_not_an_empty_response() {
        assert!(is_empty_response(
            &Message::assistant().with_content(MessageContent::thinking("", ""))
        ));
        assert!(!is_empty_response(
            &Message::assistant().with_content(MessageContent::thinking("", "sig-omitted"))
        ));
    }
}
