use anyhow::Result;
use goose_provider_types::conversation::message::{Message, MessageContent};
use goose_provider_types::conversation::token_usage::ProviderUsage;
use goose_provider_types::errors::ProviderError;
use rmcp::model::Role;
use serde::Serialize;
use tracing::warn;

use crate::format::format_message_for_compacting;
use crate::model::{CompactionModel, TokenEstimator};
use crate::structured::StructuredSummary;
use crate::templates::{render, Templates};

const REMOVAL_PERCENTAGES: [u32; 5] = [0, 10, 20, 50, 100];

const SUMMARIZE_REQUEST_TEXT: &str =
    "Please summarize the conversation history provided in the system prompt.";

#[derive(Serialize)]
struct SummarizeContext {
    messages: String,
}

#[derive(Debug)]
pub struct Summary {
    pub message: Message,
    pub usage: ProviderUsage,
}

fn has_tool_response(msg: &Message) -> bool {
    msg.content
        .iter()
        .any(|c| matches!(c, MessageContent::ToolResponse(_)))
}

/// Drops tool responses from the middle outwards, where context is least
/// likely to matter, to fit an oversized history into the summarizer.
fn filter_tool_responses(messages: &[Message], remove_percent: u32) -> Vec<&Message> {
    if remove_percent == 0 {
        return messages.iter().collect();
    }

    let tool_indices: Vec<usize> = messages
        .iter()
        .enumerate()
        .filter(|(_, msg)| has_tool_response(msg))
        .map(|(i, _)| i)
        .collect();

    if tool_indices.is_empty() {
        return messages.iter().collect();
    }

    let num_to_remove = ((tool_indices.len() * remove_percent as usize) / 100).max(1);
    let middle = tool_indices.len() / 2;
    let mut indices_to_remove = Vec::new();

    for i in 0..num_to_remove {
        let offset = i / 2;
        if i % 2 == 0 {
            if middle > offset {
                indices_to_remove.push(tool_indices[middle - offset - 1]);
            }
        } else if middle + offset < tool_indices.len() {
            indices_to_remove.push(tool_indices[middle + offset]);
        }
    }

    messages
        .iter()
        .enumerate()
        .filter(|(i, _)| !indices_to_remove.contains(i))
        .map(|(_, msg)| msg)
        .collect()
}

/// When the model didn't follow the structured output format (schema-ignoring
/// models, user-customized prompts), the raw response text is kept unchanged
/// as the summary.
fn apply_structured_summary(response: &mut Message, summary_template: &str) {
    let Some(summary) = StructuredSummary::parse(&response.as_concat_text()) else {
        return;
    };
    match summary.render_with(summary_template) {
        Ok(rendered) if !rendered.trim().is_empty() => {
            response.content = vec![MessageContent::text(rendered)];
        }
        Ok(_) => warn!(
            "Structured compaction summary rendered empty (broken template override?), keeping raw output"
        ),
        Err(e) => warn!("Failed to render structured compaction summary, keeping raw output: {e}"),
    }
}

async fn ensure_usage_tokens(
    usage: &mut ProviderUsage,
    estimator: &dyn TokenEstimator,
    system_prompt: &str,
    request: &[Message],
    response: &Message,
) {
    if usage.usage.input_tokens.is_none() {
        let count = estimator.count_chat_tokens(system_prompt, request).await;
        usage.usage.input_tokens = Some(count as i32);
    }
    if usage.usage.output_tokens.is_none() {
        let text = response
            .content
            .iter()
            .map(|c| format!("{}", c))
            .collect::<Vec<_>>()
            .join(" ");
        let count = estimator.count_text_tokens(&text).await;
        usage.usage.output_tokens = Some(count as i32);
    }
    if let (Some(input), Some(output)) = (usage.usage.input_tokens, usage.usage.output_tokens) {
        usage.usage.total_tokens = Some(input + output);
    }
}

/// Summarizes `messages` into a single user-role message, retrying with
/// progressively more tool responses removed when the summarizer itself
/// overflows its context window.
pub async fn summarize(
    model: &dyn CompactionModel,
    estimator: Option<&dyn TokenEstimator>,
    templates: &Templates,
    messages: &[Message],
) -> Result<Summary> {
    let request = vec![Message::user().with_text(SUMMARIZE_REQUEST_TEXT)];
    let has_tool_responses = messages.iter().any(has_tool_response);

    for (attempt, &remove_percent) in REMOVAL_PERCENTAGES.iter().enumerate() {
        let filtered = filter_tool_responses(messages, remove_percent);
        let context = SummarizeContext {
            messages: filtered
                .iter()
                .map(|&msg| format_message_for_compacting(msg))
                .collect::<Vec<_>>()
                .join("\n"),
        };
        let system_prompt = render(&templates.compaction, &context)?;

        match model.complete(&system_prompt, &request).await {
            Ok((mut response, mut usage)) => {
                response.role = Role::User;

                // Usage must reflect the raw model output (billable tokens),
                // so estimate before the response is rewritten to the smaller
                // rendered summary.
                if let Some(estimator) = estimator {
                    ensure_usage_tokens(&mut usage, estimator, &system_prompt, &request, &response)
                        .await;
                }

                apply_structured_summary(&mut response, &templates.summary);

                return Ok(Summary {
                    message: response,
                    usage,
                });
            }
            Err(ProviderError::ContextLengthExceeded(_)) if !has_tool_responses => {
                return Err(anyhow::anyhow!(
                    "Failed to compact: the base prompt (system prompt, tool schemas, and conversation) exceeds the model's effective context window, and there are no tool responses to remove. Use a model or configuration with a larger usable context, disable some extensions to reduce the tool-schema payload, or start a new session."
                ));
            }
            Err(ProviderError::ContextLengthExceeded(_))
                if attempt < REMOVAL_PERCENTAGES.len() - 1 => {}
            Err(ProviderError::ContextLengthExceeded(_)) => {
                return Err(anyhow::anyhow!(
                    "Failed to compact: context limit exceeded even after removing all tool responses"
                ));
            }
            Err(e) => return Err(e.into()),
        }
    }

    Err(anyhow::anyhow!(
        "Unexpected: exhausted all attempts without returning"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::CompactionModel;
    use crate::templates::Templates;
    use async_trait::async_trait;
    use rmcp::model::CallToolResult;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct OverflowingModel {
        request_count: AtomicUsize,
    }

    impl OverflowingModel {
        fn new() -> Self {
            Self {
                request_count: AtomicUsize::new(0),
            }
        }

        fn request_count(&self) -> usize {
            self.request_count.load(Ordering::Relaxed)
        }
    }

    #[async_trait]
    impl CompactionModel for OverflowingModel {
        async fn complete(
            &self,
            _system: &str,
            _messages: &[Message],
        ) -> Result<(Message, ProviderUsage), ProviderError> {
            self.request_count.fetch_add(1, Ordering::Relaxed);
            Err(ProviderError::ContextLengthExceeded(
                "Prompt exceeds context limit".to_string(),
            ))
        }
    }

    #[tokio::test]
    async fn summarize_without_tool_responses_fails_fast() {
        let model = OverflowingModel::new();
        let messages = vec![Message::user().with_text("oversized conversation")];

        let error = summarize(&model, None, &Templates::default(), &messages)
            .await
            .unwrap_err();
        let error_message = error.to_string();

        assert_eq!(model.request_count(), 1);
        assert!(error_message.contains("there are no tool responses to remove"));
        assert!(!error_message.contains("even after removing all tool responses"));
        assert!(error_message.contains("larger usable context"));
        assert!(error_message.contains("disable some extensions"));
        assert!(error_message.contains("start a new session"));
    }

    #[tokio::test]
    async fn summarize_with_tool_responses_preserves_exhausted_removal_error() {
        let model = OverflowingModel::new();
        let messages = vec![
            Message::user().with_text("please read the file"),
            Message::user().with_tool_response(
                "tool_0",
                Ok(CallToolResult::success(vec![
                    rmcp::model::ContentBlock::text("contents"),
                ])),
            ),
        ];

        let error = summarize(&model, None, &Templates::default(), &messages)
            .await
            .unwrap_err();

        assert_eq!(model.request_count(), REMOVAL_PERCENTAGES.len());
        assert_eq!(
            error.to_string(),
            "Failed to compact: context limit exceeded even after removing all tool responses"
        );
    }
}
