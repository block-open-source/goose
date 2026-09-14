use anyhow::Result;
use goose_providers::conversation::token_usage::{ProviderUsage, Usage as ProviderTokenUsage};
use rmcp::model::{CallToolRequestParams, CallToolResult, ContentBlock};

use super::calculator_extension::{value, ADD};
use super::dummy_api::ProviderFeatures;
use super::pipeline::{self, test_pipeline, MessageKind::Agent};
use crate::agents::state_machine;
use crate::agents::state_machine::ops_compaction::MAX_CONTEXT_ERROR_COMPACTIONS;
use crate::context_mgmt::{compute_tool_call_cutoff, TOOLCALL_SUMMARIZATION_BATCH_SIZE};
use crate::conversation::message::{Message, MessageErrorKind};
use crate::conversation::Conversation;

const SUMMARIZE_HISTORY: &str = "Please summarize the conversation history";
const SUMMARIZE_TOOL_PAIR: &str = "summarize a tool call & response pair";

#[tokio::test]
async fn proactive_and_manual_compaction_continue_with_replaced_usage() -> Result<()> {
    let (pipeline, api) = test_pipeline().await?;
    api.on("fill the context").reply("filled");
    api.on("check the budget").reply("budget checked");
    api.on(SUMMARIZE_HISTORY).reply("summary");
    api.on("Your context was compacted")
        .reply("continued after compaction");
    api.on("after manual compaction").reply("still working");

    let half_full = format!(
        "fill the context {}",
        "x".repeat(pipeline.context_limit() / 2)
    );
    pipeline.run([half_full.as_str()]).await?;
    let budget = pipeline.run(["check the budget"]).await?;
    budget.assert_message(-1, Agent, "budget checked");
    assert!(api.calls().last().unwrap().input_contains("<compaction>"));

    let filled_usage = (pipeline.context_limit() as f64 * 0.81) as i32;
    pipeline.set_total_tokens(filled_usage).await;
    let compacted = pipeline.run(["continue"]).await?;
    compacted.assert_message(-1, Agent, "continued after compaction");
    compacted.assert_emitted("Performing auto-compaction");
    assert_eq!(compacted.history_replacements(), 1);
    assert!(compacted
        .session
        .usage
        .total_tokens
        .is_some_and(|tokens| tokens < filled_usage));

    let first_manual = pipeline.run(["/compact"]).await?;
    let second_manual = pipeline.run(["/compact"]).await?;
    first_manual.assert_emitted("Compaction complete");
    assert_eq!(first_manual.history_replacements(), 1);
    assert_eq!(second_manual.history_replacements(), 1);

    let commands = second_manual
        .conversation()
        .messages()
        .iter()
        .filter(|message| message.as_concat_text().trim() == "/compact")
        .collect::<Vec<_>>();
    assert_eq!(commands.len(), 2);
    assert!(commands
        .iter()
        .all(|message| message.is_user_visible() && !message.is_agent_visible()));

    let continued = pipeline.run(["after manual compaction"]).await?;
    continued.assert_message(-1, Agent, "still working");

    pipeline.set_total_tokens(100).await;
    let cleared = pipeline.run(["/clear"]).await?;
    assert_eq!(cleared.history_replacements(), 1);
    assert_eq!(cleared.conversation().messages().len(), 2);
    assert!(cleared
        .conversation()
        .messages()
        .iter()
        .all(|message| message.is_user_visible() && !message.is_agent_visible()));
    assert_eq!(cleared.session.usage.total_tokens, Some(0));

    let (pipeline, _api) = test_pipeline().await?;
    pipeline.set_total_tokens(100).await;
    let machine =
        state_machine::StateMachine::new(Vec::new(), tokio_util::sync::CancellationToken::new());
    let (tx, _rx) = tokio::sync::mpsc::channel(4);
    let emit = state_machine::Emitter::new(tx, tokio_util::sync::CancellationToken::new());
    let apply = async |effects: Vec<state_machine::GooseEffect>| -> Result<()> {
        let session = pipeline.session().await?;
        let mut result = state_machine::StepResult {
            effects,
            applied_step: None,
            yield_to_client: false,
        };
        machine
            .apply(
                pipeline.session_manager.as_ref(),
                &session,
                &mut result,
                &emit,
            )
            .await
    };

    let replacement = Conversation::new_unvalidated([
        Message::user().with_text("keep this"),
        Message::assistant().with_text("and this"),
    ]);
    apply(vec![replacement.into()]).await?;
    let recounted = pipeline.session().await?.usage.total_tokens;
    assert!(recounted.is_some_and(|tokens| tokens > 0 && tokens < 100));

    let replacement = Conversation::new_unvalidated([Message::user().with_text("new context")]);
    let usage = ProviderUsage::new(
        "scripted-model".to_string(),
        ProviderTokenUsage::new(Some(10), Some(5), Some(15)),
    );
    apply(vec![
        replacement.into(),
        state_machine::GooseEffect::RecordUsage(usage),
        Message::assistant()
            .with_text("response after replacement")
            .into(),
    ])
    .await?;

    let reloaded = pipeline.session().await?;
    assert_eq!(reloaded.usage.total_tokens, Some(15));
    assert_eq!(
        reloaded
            .conversation
            .and_then(|conversation| conversation.last().cloned())
            .and_then(|message| message.metadata.usage)
            .and_then(|usage| usage.total_tokens),
        Some(15)
    );

    Ok(())
}

#[tokio::test]
async fn tokenless_provider_compacts_estimated_context() -> Result<()> {
    let (pipeline, api) = pipeline::test_pipeline_with(ProviderFeatures {
        reports_usage: false,
        ..ProviderFeatures::default()
    })
    .await?;
    let pipeline = pipeline
        .with_model_config(
            goose_providers::model::ModelConfig::new("gpt-4.1").with_context_limit(Some(200)),
        )
        .await;
    let large_context = (0..500)
        .map(|index| format!("token-{index}"))
        .collect::<Vec<_>>()
        .join(" ");
    pipeline
        .seed([Message::user().with_text(large_context)])
        .await?;
    assert!(pipeline.session().await?.usage.total_tokens.is_none());

    api.on(SUMMARIZE_HISTORY).reply("summary");
    api.on("Your context was compacted")
        .reply("continued after estimated compaction");

    let compacted = pipeline.run(["continue"]).await?;
    compacted.assert_message(-1, Agent, "continued after estimated compaction");
    compacted.assert_emitted("Performing auto-compaction");
    assert_eq!(compacted.history_replacements(), 1);

    Ok(())
}

#[tokio::test]
async fn a_failed_compact_command_reports_the_error_and_keeps_working() -> Result<()> {
    let (pipeline, api) = test_pipeline().await?;
    api.on("do some work").reply("worked");
    api.on(SUMMARIZE_HISTORY).server_error("summarizer offline");
    pipeline.run(["do some work"]).await?;

    let failed = pipeline.run(["/compact"]).await?;
    assert_eq!(failed.history_replacements(), 0);
    failed.assert_message(-1, Agent, "summarizer offline");
    assert!(failed
        .conversation()
        .messages()
        .iter()
        .any(|message| message.as_concat_text() == "worked"));

    api.on("still there?").reply("still here");
    let recovered = pipeline.run(["still there?"]).await?;
    recovered.assert_message(-1, Agent, "still here");

    Ok(())
}

#[tokio::test]
async fn context_owning_provider_has_no_compaction_operation() -> Result<()> {
    let (pipeline, api) = pipeline::test_pipeline_with(ProviderFeatures {
        manages_own_context: true,
        ..ProviderFeatures::default()
    })
    .await?;
    api.on("continue").reply("continued");
    pipeline
        .set_total_tokens((pipeline.context_limit() as f64 * 0.81) as i32)
        .await;

    let continued = pipeline.run(["continue"]).await?;
    continued.assert_message(-1, Agent, "continued");
    assert_eq!(continued.history_replacements(), 0);
    assert_eq!(api.calls().len(), 1);

    for command in ["clear", "compact"] {
        let input = format!("/{command}");
        api.on(&input).reply(format!("provider handled /{command}"));
        let handled = pipeline.run([input.as_str()]).await?;
        handled.assert_message(-1, Agent, &format!("provider handled /{command}"));
        assert_eq!(handled.history_replacements(), 0);
    }
    assert_eq!(api.calls().len(), 3);

    Ok(())
}

#[tokio::test]
async fn text_that_looks_like_a_context_error_does_not_compact() -> Result<()> {
    let (pipeline, api) = test_pipeline().await?;
    api.on("false alarm")
        .reply("the provider said context length exceeded, but this is ordinary text");

    let false_alarm = pipeline.run(["false alarm"]).await?;
    false_alarm.assert_message(-1, Agent, "context length exceeded");
    assert_eq!(false_alarm.history_replacements(), 0);

    Ok(())
}

#[tokio::test]
async fn a_context_error_compacts_and_the_session_survives_a_failed_retry() -> Result<()> {
    let (pipeline, api) = test_pipeline().await?;
    api.on("real error").context_limit_error("too long");
    api.on(SUMMARIZE_HISTORY).reply("summary");
    api.on("Your context was compacted")
        .server_error("provider unavailable");

    let failed_after_compaction = pipeline.run(["real error"]).await?;
    assert_eq!(failed_after_compaction.history_replacements(), 1);
    assert_eq!(
        failed_after_compaction
            .conversation()
            .last()
            .and_then(Message::error_kind),
        Some(MessageErrorKind::Other)
    );

    api.on("try again").reply("recovered on the next turn");
    let recovered = pipeline.run(["try again"]).await?;
    recovered.assert_message(-1, Agent, "recovered on the next turn");

    Ok(())
}

#[tokio::test]
async fn repeated_context_errors_stop_compacting() -> Result<()> {
    let (pipeline, api) = test_pipeline().await?;
    api.on("keep overflowing").context_limit_error("too long");
    api.on(SUMMARIZE_HISTORY).reply("summary");
    api.on("Your context was compacted")
        .context_limit_error("too long");

    let mut kickoff = Message::user().with_text("keep overflowing");
    kickoff.created = 1;
    let capped = pipeline.run_message(kickoff).await?;

    assert_eq!(capped.history_replacements(), MAX_CONTEXT_ERROR_COMPACTIONS);
    assert_eq!(api.call_count(), 1 + 2 * MAX_CONTEXT_ERROR_COMPACTIONS);
    assert_eq!(
        capped.conversation().last().and_then(Message::error_kind),
        Some(MessageErrorKind::ContextLengthExceeded)
    );

    Ok(())
}

#[tokio::test]
async fn tool_pairs_are_compacted_only_after_the_current_turn() -> Result<()> {
    let (pipeline, api) = test_pipeline().await?;
    let cutoff = compute_tool_call_cutoff(pipeline.context_limit(), pipeline::COMPACTION_THRESHOLD);
    let boundary = cutoff + TOOLCALL_SUMMARIZATION_BATCH_SIZE;

    api.on("do a lot of work").call(ADD, value(1));
    api.on("result:").call(ADD, value(1));
    api.on(format!("result: {}", boundary - 1))
        .reply("first batch done");
    api.on("reach the boundary").call(ADD, value(1));
    api.on(format!("result: {boundary}"))
        .reply("at the boundary");
    api.on("cross the boundary").call(ADD, value(1));
    api.on(format!("result: {}", boundary + 1))
        .reply("all work done");
    api.on_system(SUMMARIZE_TOOL_PAIR)
        .reply("summary of the pair");
    api.on("carry on").reply("carried on");

    let current_turn = pipeline
        .run([
            "do a lot of work",
            "reach the boundary",
            "cross the boundary",
        ])
        .await?;
    current_turn.assert_message(-1, Agent, "all work done");
    assert_eq!(
        current_turn
            .conversation()
            .messages()
            .iter()
            .filter(|message| message.is_agent_visible() && message.is_tool_call())
            .count(),
        boundary + 1
    );
    assert!(!current_turn
        .conversation()
        .messages()
        .iter()
        .any(|message| message.as_concat_text() == "summary of the pair"));

    let calls_before = api.call_count();
    let next_turn = pipeline.run(["carry on"]).await?;
    next_turn.assert_message(-1, Agent, "carried on");

    // The batch is one provider call per pair plus the turn's own inference. A
    // pair whose summary call fails is left alone, so check that every call was
    // made before reading the counts it produced.
    assert_eq!(
        api.call_count() - calls_before,
        TOOLCALL_SUMMARIZATION_BATCH_SIZE + 1,
        "expected a summary request per pair"
    );
    let summaries = next_turn
        .conversation()
        .messages()
        .iter()
        .filter(|message| {
            message.as_concat_text() == "summary of the pair"
                && message.is_agent_visible()
                && !message.is_user_visible()
        })
        .count();
    let visible_tool_calls = next_turn
        .conversation()
        .messages()
        .iter()
        .filter(|message| message.is_agent_visible() && message.is_tool_call())
        .count();
    assert_eq!(
        (summaries, visible_tool_calls),
        (
            TOOLCALL_SUMMARIZATION_BATCH_SIZE,
            boundary + 1 - TOOLCALL_SUMMARIZATION_BATCH_SIZE
        ),
        "summaries and the tool calls they replaced disagree"
    );

    Ok(())
}

#[tokio::test]
async fn parallel_and_failed_tool_pairs_are_compacted_as_complete_messages() -> Result<()> {
    let (pipeline, api) = test_pipeline().await?;
    let cutoff = compute_tool_call_cutoff(pipeline.context_limit(), pipeline::COMPACTION_THRESHOLD);
    let calls_per_message = 2;
    let batches = 2;
    let summarized_messages = batches * TOOLCALL_SUMMARIZATION_BATCH_SIZE / calls_per_message;
    let pairs =
        (cutoff + batches * TOOLCALL_SUMMARIZATION_BATCH_SIZE).div_ceil(calls_per_message) + 1;

    api.on_system(SUMMARIZE_TOOL_PAIR).reply("pair summary");
    api.on("carry on").reply("done");

    pipeline
        .seed([Message::user().with_text("old work")])
        .await?;
    for n in 0..pairs {
        let ids = [format!("call_{n}a"), format!("call_{n}b")];
        let mut request = Message::assistant();
        let mut response = Message::user();
        for id in &ids {
            request = request.with_tool_request(
                id.clone(),
                Ok(CallToolRequestParams::new(ADD).with_arguments(serde_json::Map::new())),
            );
            let result = if n == 0 {
                CallToolResult::error(vec![ContentBlock::text("failed calculation")])
            } else {
                CallToolResult::success(vec![ContentBlock::text("result")])
            };
            response.add_tool_response_with_metadata(id.clone(), Ok(result), None);
        }
        pipeline.seed([request, response]).await?;
    }

    let result = pipeline.run(["carry on"]).await?;
    result.assert_message(-1, Agent, "done");
    assert_eq!(api.call_count(), summarized_messages + 1);

    let persisted = result.conversation();
    assert_eq!(
        persisted
            .messages()
            .iter()
            .filter(|message| {
                message.as_concat_text() == "pair summary"
                    && message.is_agent_visible()
                    && !message.is_user_visible()
            })
            .count(),
        summarized_messages
    );
    let failed_pair = persisted
        .messages()
        .iter()
        .filter(|message| {
            message.get_tool_request_ids().contains("call_0a")
                || message.get_tool_response_ids().contains("call_0a")
        })
        .collect::<Vec<_>>();
    assert_eq!(failed_pair.len(), 2);
    assert!(failed_pair
        .iter()
        .all(|message| message.is_user_visible() && !message.is_agent_visible()));
    for message in persisted
        .messages()
        .iter()
        .filter(|message| message.is_tool_response())
    {
        let response_ids = message.get_tool_response_ids();
        let paired_visibility = persisted
            .messages()
            .iter()
            .find(|request| {
                request
                    .get_tool_request_ids()
                    .intersection(&response_ids)
                    .next()
                    .is_some()
            })
            .map(Message::is_agent_visible);
        assert_eq!(paired_visibility, Some(message.is_agent_visible()));
    }

    Ok(())
}

#[tokio::test]
async fn a_small_model_compacts_a_large_tool_result_out_of_the_conversation() -> Result<()> {
    let (pipeline, api) = test_pipeline().await?;
    let pipeline = pipeline.with_model("gpt-3.5-turbo").await;
    let large_result = "x".repeat(2_000);

    let request = Message::assistant().with_tool_request(
        "large-result",
        Ok(CallToolRequestParams::new(ADD).with_arguments(serde_json::Map::new())),
    );
    let mut response = Message::user();
    response.add_tool_response_with_metadata(
        "large-result",
        Ok(CallToolResult::success(vec![ContentBlock::text(
            large_result.clone(),
        )])),
        None,
    );
    pipeline
        .seed([Message::user().with_text("old work"), request, response])
        .await?;
    let filled_usage = (pipeline.context_limit() as f64 * 0.85) as i32;
    pipeline.set_total_tokens(filled_usage).await;

    api.on(SUMMARIZE_HISTORY).reply("large work summarized");
    api.on("Your context was compacted").reply("continued");

    let compacted = pipeline.run(["continue"]).await?;
    compacted.assert_message(-1, Agent, "continued");
    assert_eq!(compacted.history_replacements(), 1);
    assert!(compacted
        .session
        .usage
        .total_tokens
        .is_some_and(|tokens| tokens < filled_usage));

    let summarization = api
        .calls()
        .into_iter()
        .find(|call| call.input_contains(SUMMARIZE_HISTORY))
        .expect("summarization request");
    assert!(summarization.system_contains(&large_result));
    assert!(!api.calls().last().unwrap().input_contains(&large_result));
    assert!(!compacted
        .conversation()
        .agent_visible_messages()
        .iter()
        .any(|message| message.as_concat_text().contains(&large_result)));

    Ok(())
}
