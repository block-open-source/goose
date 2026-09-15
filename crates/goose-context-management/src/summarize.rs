use anyhow::Result;
use goose_provider_types::conversation::message::{Message, MessageContent};
use goose_provider_types::conversation::token_usage::ProviderUsage;
use goose_provider_types::errors::ProviderError;
use rmcp::model::{Role, Tool};
use serde::Serialize;
use tracing::warn;

use crate::format::format_message_for_compacting;
use crate::model::{CompactionModel, TokenEstimator};
use crate::structured::StructuredSummary;
use crate::templates::{render, Templates};

const REMOVAL_PERCENTAGES: [u32; 5] = [0, 10, 20, 50, 100];

const ELIDED_TOOL_RESPONSE_TEXT: &str =
    "[tool response elided: the conversation exceeded the summarizer's context window]";

/// Room left for the summary itself and for the estimator being approximate.
const SUMMARY_RESERVE_FRACTION: f64 = 0.1;

/// Kept in wording from the standalone path (#10500): the actions that
/// actually recover a session are the same ones.
const CONTEXT_EXHAUSTED_TEXT: &str =
    "Failed to compact: the conversation exceeds the model's effective context window even with \
     every tool response elided and the system prompt and tool schemas dropped. Use a model or \
     configuration with a larger usable context, disable some extensions to reduce the \
     tool-schema payload, or start a new session.";

const TOOL_CALL_NOT_EXECUTED_TEXT: &str =
    "This tool call was not executed: tools are unavailable while summarizing.";

const REJECTED_SUMMARY_RETRY_TEXT: &str = "That reply was not the requested summary. Reply now \
    with the structured summary exactly as instructed above, without calling tools.";

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

/// Message indices of tool responses to sacrifice, chosen from the middle
/// outwards, where context is least likely to matter.
fn middle_out_tool_response_indices(messages: &[Message], remove_percent: u32) -> Vec<usize> {
    if remove_percent == 0 {
        return Vec::new();
    }

    let tool_indices: Vec<usize> = messages
        .iter()
        .enumerate()
        .filter(|(_, msg)| has_tool_response(msg))
        .map(|(i, _)| i)
        .collect();

    if tool_indices.is_empty() {
        return Vec::new();
    }

    let num_to_remove = ((tool_indices.len() * remove_percent as usize) / 100).max(1);
    let center = (tool_indices.len() as f64 - 1.0) / 2.0;
    let mut positions: Vec<usize> = (0..tool_indices.len()).collect();
    positions.sort_by(|&a, &b| {
        (a as f64 - center)
            .abs()
            .total_cmp(&(b as f64 - center).abs())
            .then(a.cmp(&b))
    });

    positions
        .into_iter()
        .take(num_to_remove)
        .map(|position| tool_indices[position])
        .collect()
}

fn filter_tool_responses(messages: &[Message], remove_percent: u32) -> Vec<&Message> {
    let indices_to_remove = middle_out_tool_response_indices(messages, remove_percent);
    messages
        .iter()
        .enumerate()
        .filter(|(i, _)| !indices_to_remove.contains(i))
        .map(|(_, msg)| msg)
        .collect()
}

/// Unlike dropping the message, eliding its response contents keeps the tool
/// request/response pair intact, which providers require of a native-shape
/// request.
fn elide_tool_responses(msg: &Message) -> Message {
    let mut msg = msg.clone();
    for content in &mut msg.content {
        if let MessageContent::ToolResponse(response) = content {
            response.tool_result = Ok(rmcp::model::CallToolResult::success(vec![
                rmcp::model::ContentBlock::text(ELIDED_TOOL_RESPONSE_TEXT),
            ]));
        }
    }
    msg
}

fn elide_tool_responses_at(messages: &[Message], indices_to_elide: &[usize]) -> Vec<Message> {
    messages
        .iter()
        .enumerate()
        .map(|(i, msg)| {
            if indices_to_elide.contains(&i) {
                elide_tool_responses(msg)
            } else {
                msg.clone()
            }
        })
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
    tools: &[Tool],
    response: &Message,
) {
    if usage.usage.input_tokens.is_none() {
        let count = estimator
            .count_chat_tokens_with_tools(system_prompt, request, tools)
            .await;
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

/// Extends the rejected request in place rather than rebuilding it, keeping
/// the already-cached prefix intact.
fn correction_request(mut request: Vec<Message>, response: &Message) -> Vec<Message> {
    let tool_call_ids: Vec<String> = response
        .content
        .iter()
        .filter_map(|content| match content {
            MessageContent::ToolRequest(tool_request) => Some(tool_request.id.clone()),
            _ => None,
        })
        .collect();

    if tool_call_ids.is_empty() {
        if let Some(instruction) = request.last_mut() {
            instruction
                .content
                .push(MessageContent::text(REJECTED_SUMMARY_RETRY_TEXT));
        }
        return request;
    }

    request.push(response.clone());
    let mut follow_up = Message::user();
    for id in tool_call_ids {
        follow_up = follow_up.with_tool_response(
            id,
            Ok(rmcp::model::CallToolResult::success(vec![
                rmcp::model::ContentBlock::text(TOOL_CALL_NOT_EXECUTED_TEXT),
            ])),
        );
    }
    request.push(follow_up.with_text(REJECTED_SUMMARY_RETRY_TEXT));
    request
}

/// A failed compaction; carries the billed usage of each
/// completed-but-rejected call so callers can still account for them.
#[derive(Debug)]
pub struct CompactionFailure {
    pub error: anyhow::Error,
    pub billed_usage: Vec<ProviderUsage>,
}

impl std::fmt::Display for CompactionFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.error.fmt(f)
    }
}

impl std::error::Error for CompactionFailure {}

/// One way of making the summarization request fit, ordered by what it costs.
/// Eliding tool responses forfeits the provider's cache only from the first
/// elided message onwards; dropping the header forfeits all of it.
#[derive(PartialEq, Eq)]
struct FitPlan {
    drop_header: bool,
    elide: Vec<usize>,
}

impl FitPlan {
    fn as_is() -> Self {
        Self {
            drop_header: false,
            elide: Vec::new(),
        }
    }

    fn apply(&self, messages: &[Message]) -> Vec<Message> {
        if self.elide.is_empty() {
            messages.to_vec()
        } else {
            elide_tool_responses_at(messages, &self.elide)
        }
    }
}

/// Elides tool responses from the middle outwards until the request is
/// projected to fit, so the number elided follows the overflow rather than a
/// fixed percentage. `None` when eliding every one of them is not enough.
async fn elisions_covering(
    estimator: &dyn TokenEstimator,
    deficit: usize,
    messages: &[Message],
    candidates: &[usize],
) -> Option<Vec<usize>> {
    let mut saved = 0;
    let mut chosen = Vec::new();
    for &index in candidates {
        if saved >= deficit {
            break;
        }
        let message = std::slice::from_ref(&messages[index]);
        let elided = [elide_tool_responses(&messages[index])];
        saved += estimator
            .count_chat_tokens("", message)
            .await
            .saturating_sub(estimator.count_chat_tokens("", &elided).await);
        chosen.push(index);
    }
    (saved >= deficit).then_some(chosen)
}

/// The least destructive plan the estimate says fits for a given header
/// choice, or `None` when eliding every tool response still is not enough.
async fn covering_plan(
    estimator: &dyn TokenEstimator,
    budget: usize,
    system: &str,
    tools: &[Tool],
    messages: &[Message],
    drop_header: bool,
) -> Option<FitPlan> {
    let (system, tools): (&str, &[Tool]) = if drop_header {
        ("", &[])
    } else {
        (system, tools)
    };
    let total = estimator
        .count_chat_tokens_with_tools(system, messages, tools)
        .await;
    if total <= budget {
        return Some(FitPlan {
            drop_header,
            elide: Vec::new(),
        });
    }
    let candidates = middle_out_tool_response_indices(messages, 100);
    elisions_covering(estimator, total - budget, messages, &candidates)
        .await
        .map(|elide| FitPlan { drop_header, elide })
}

/// The escalation sequence to try, least destructive first. The first entry is
/// the measured one; the rest cover estimator error, since the provider is the
/// final authority on what fits. The estimator undercounts thinking blocks and
/// images, so the fallback rungs stay graduated rather than jumping straight to
/// eliding everything.
///
/// When the header is what does not fit, it is given back before any tool
/// response is: the header costs nothing to rebuild, whereas an elided response
/// is gone from the summary for good.
async fn fit_plans(
    estimator: Option<&dyn TokenEstimator>,
    budget: Option<usize>,
    system: &str,
    tools: &[Tool],
    messages: &[Message],
) -> Vec<FitPlan> {
    // Dropping a header that is already empty produces the same request with a
    // different flag, which `dedup` cannot collapse: the identical request then
    // goes out twice. Only claim to drop one when there is one.
    let has_header = !system.is_empty() || !tools.is_empty();
    let headerless = FitPlan {
        drop_header: has_header,
        elide: middle_out_tool_response_indices(messages, 100),
    };

    let measured = match estimator.zip(budget) {
        None => FitPlan::as_is(),
        Some((estimator, budget)) => {
            match covering_plan(estimator, budget, system, tools, messages, false).await {
                Some(plan) => plan,
                None => {
                    let mut plans =
                        match covering_plan(estimator, budget, system, tools, messages, true).await
                        {
                            Some(plan) => vec![plan, headerless],
                            None => vec![headerless],
                        };
                    plans.dedup();
                    return plans;
                }
            }
        }
    };

    let measured_count = measured.elide.len();
    let mut plans = vec![measured];
    plans.extend(
        REMOVAL_PERCENTAGES
            .iter()
            .filter(|&&percent| percent < 100)
            .map(|&percent| FitPlan {
                drop_header: false,
                elide: middle_out_tool_response_indices(messages, percent),
            })
            .filter(|plan| plan.elide.len() > measured_count),
    );
    // Before sacrificing every tool response to keep the header, try keeping
    // every response without it. Pointless when there is no header to give
    // back, which is the same request the ladder already sent.
    if has_header {
        plans.push(FitPlan {
            drop_header: true,
            elide: Vec::new(),
        });
    }
    plans.push(headerless);
    plans.dedup();
    plans
}

/// Summarizes by replaying the conversation's own request prefix (system,
/// tools, messages as the provider last saw them, instruction last) so the
/// provider's prompt cache is reused. The request is measured against the
/// model's context window first and elided to fit before it is sent. A
/// response that isn't a summary (a tool call, or no text) gets one corrective
/// retry before failing; usage of rejected attempts is carried into the
/// outcome.
pub async fn summarize_as_prefix(
    model: &dyn CompactionModel,
    estimator: Option<&dyn TokenEstimator>,
    summary_template: &str,
    system: &str,
    tools: &[Tool],
    request_messages: &[Message],
) -> Result<Summary, CompactionFailure> {
    let budget = model
        .context_limit()
        .await
        .map(|limit| limit - (limit as f64 * SUMMARY_RESERVE_FRACTION) as usize);
    let plans = fit_plans(estimator, budget, system, tools, request_messages).await;

    let mut last_overflow = None;
    let mut corrected = false;
    let mut correction_in_flight = false;
    let mut rejected_usage: Vec<ProviderUsage> = Vec::new();
    for plan in plans {
        let (system, tools): (&str, &[Tool]) = if plan.drop_header {
            ("", &[])
        } else {
            (system, tools)
        };
        let mut request = plan.apply(request_messages);

        loop {
            let (mut response, mut usage) =
                match model.complete_prefix(system, &request, tools).await {
                    Ok(completed) => completed,
                    Err(ProviderError::ContextLengthExceeded(error)) => {
                        last_overflow = Some(error);
                        // An overflowing corrective request must not use up
                        // the correction: the next plan retries uncorrected.
                        if correction_in_flight {
                            corrected = false;
                            correction_in_flight = false;
                        }
                        break;
                    }
                    Err(error) => {
                        return Err(CompactionFailure {
                            error: error.into(),
                            billed_usage: rejected_usage,
                        })
                    }
                };

            // Estimate before the rejection checks so rejected usage is
            // still complete.
            if let Some(estimator) = estimator {
                ensure_usage_tokens(&mut usage, estimator, system, &request, tools, &response)
                    .await;
            }

            let rejection = if response
                .content
                .iter()
                .any(|content| matches!(content, MessageContent::ToolRequest(_)))
            {
                Some("summarizer called a tool instead of producing a summary")
            } else if response.as_concat_text().trim().is_empty() {
                Some("summarization produced no text content")
            } else {
                None
            };
            if let Some(reason) = rejection {
                rejected_usage.push(usage);
                if corrected {
                    return Err(CompactionFailure {
                        error: anyhow::anyhow!("{reason}"),
                        billed_usage: rejected_usage,
                    });
                }
                corrected = true;
                correction_in_flight = true;
                request = correction_request(request, &response);
                continue;
            }

            for prior in rejected_usage.drain(..) {
                usage.usage += prior.usage;
                usage.cost = match (usage.cost, prior.cost) {
                    (Some(current), Some(previous)) => Some(current + previous),
                    (current, previous) => current.or(previous),
                };
            }

            response.role = Role::User;
            // The session may run with extended thinking; only the text
            // carries the summary.
            response
                .content
                .retain(|content| matches!(content, MessageContent::Text(_)));
            apply_structured_summary(&mut response, summary_template);

            return Ok(Summary {
                message: response,
                usage,
            });
        }
    }

    Err(CompactionFailure {
        error: anyhow::Error::new(ProviderError::ContextLengthExceeded(
            last_overflow.unwrap_or_default(),
        ))
        .context(CONTEXT_EXHAUSTED_TEXT),
        billed_usage: rejected_usage,
    })
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
                    ensure_usage_tokens(
                        &mut usage,
                        estimator,
                        &system_prompt,
                        &request,
                        &[],
                        &response,
                    )
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

        async fn complete_prefix(
            &self,
            system: &str,
            messages: &[Message],
            _tools: &[Tool],
        ) -> Result<(Message, ProviderUsage), ProviderError> {
            self.complete(system, messages).await
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

    /// Without a recorded header, dropping the header is a no-op, so the
    /// headerless fallback is the request the ladder already sent.
    #[tokio::test]
    async fn headerless_fallback_does_not_repeat_the_first_request() {
        let model = OverflowingModel::new();
        let messages = vec![Message::user().with_text("oversized conversation")];

        summarize_as_prefix(&model, None, "summarize this", "", &[], &messages)
            .await
            .unwrap_err();

        assert_eq!(model.request_count(), 1);
    }
}
