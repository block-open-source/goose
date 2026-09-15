use anyhow::Result;
use goose_providers::conversation::token_usage::{ProviderUsage, Usage as ProviderTokenUsage};
use rmcp::model::{CallToolRequestParams, CallToolResult, ContentBlock};
use serde_json::json;

use super::calculator_extension::{value, ADD};
use super::dummy_api::ProviderFeatures;
use super::pipeline::{self, test_pipeline, MessageKind::Agent};
use crate::agents::state_machine;
use crate::agents::state_machine::ops_compaction::MAX_CONTEXT_ERROR_COMPACTIONS;
use crate::context_mgmt::{compute_tool_call_cutoff, TOOLCALL_SUMMARIZATION_BATCH_SIZE};
use crate::conversation::message::{Message, MessageErrorKind, MessageMetadata, MessageUsage};
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
async fn a_tool_pair_replacement_is_not_charged_to_the_stale_baseline() -> Result<()> {
    let (pipeline, api) = test_pipeline().await?;
    // Tool-pair summarization hides ten old pairs and appends their
    // summaries behind the provider's latest response. The session baseline
    // still counts the hidden pairs, so adding the summaries to it without
    // subtracting the pairs would cross the 19.2k threshold and compact a
    // context that just shrank far below it. The recount must subtract the
    // pairs the summaries hid. gpt-4.1's ~1M canonical limit keeps the
    // API's rejection wall far above every request.
    let pipeline = pipeline
        .with_model_config(
            goose_providers::model::ModelConfig::new("gpt-4.1").with_context_limit(Some(24_000)),
        )
        .await;

    let mut history = vec![Message::user().with_text("run the tools")];
    let pairs = compute_tool_call_cutoff(pipeline.context_limit(), pipeline::COMPACTION_THRESHOLD)
        + TOOLCALL_SUMMARIZATION_BATCH_SIZE
        + 1;
    for i in 0..pairs as u32 {
        // The ten summarized pairs are large and their summaries small, so
        // the replacement genuinely shrinks the context: ~1.2k tokens of
        // response hidden per pair against ~500 added back.
        let result = if i < TOOLCALL_SUMMARIZATION_BATCH_SIZE as u32 {
            "x ".repeat(1_200)
        } else {
            "1".to_string()
        };
        history.push(
            Message::assistant()
                .with_id(format!("req-{i}"))
                .with_tool_request(
                    format!("call-{i}"),
                    Ok(CallToolRequestParams::new(ADD).with_arguments(serde_json::Map::new())),
                ),
        );
        history.push(Message::user().with_tool_response(
            format!("call-{i}"),
            Ok(CallToolResult::success(vec![ContentBlock::text(result)])),
        ));
    }
    history.push(
        Message::assistant()
            .with_id("final")
            .with_text("all tools ran")
            .with_metadata(MessageMetadata {
                usage: Some(Box::new(MessageUsage::default())),
                ..MessageMetadata::default()
            }),
    );
    pipeline.seed_messages(history).await?;
    pipeline.set_total_tokens(18_000).await;

    // Charging the ten ~500-token summaries to the 18k baseline on top of
    // the pairs it still counts would reach 23k, over the 19.2k threshold;
    // subtracting the ~12k of hidden pairs leaves the estimate near 10.7k,
    // under it.
    api.on_system(SUMMARIZE_TOOL_PAIR).reply("x ".repeat(500));
    api.on(SUMMARIZE_HISTORY).reply("history summarized");
    api.on("Your context was compacted")
        .reply("compacted anyway");
    api.on("carry on").reply("carried on");

    let run = pipeline.run(["carry on"]).await?;

    run.assert_message(-1, Agent, "carried on");
    assert_eq!(
        run.history_replacements(),
        0,
        "a shrunk context must not be compacted on its stale baseline"
    );
    assert!(
        !api.calls()
            .into_iter()
            .any(|call| call.input_contains(SUMMARIZE_HISTORY)),
        "no summarization may run at the replacement boundary"
    );
    let summaries = run
        .conversation()
        .messages()
        .iter()
        .filter(|message| message.is_agent_visible() && !message.is_user_visible())
        .filter(|message| message.as_concat_text().contains("x x"))
        .count();
    assert_eq!(summaries, TOOLCALL_SUMMARIZATION_BATCH_SIZE);
    let visible_tool_calls = run
        .conversation()
        .messages()
        .iter()
        .filter(|message| message.is_agent_visible() && message.is_tool_call())
        .count();
    assert_eq!(
        visible_tool_calls,
        pairs - TOOLCALL_SUMMARIZATION_BATCH_SIZE,
        "the replaced pairs must be hidden, the rest kept"
    );
    assert_eq!(api.context_limit_rejections(), 0);

    Ok(())
}

#[tokio::test]
async fn a_tool_pair_replacement_preserves_the_baseline_overhead() -> Result<()> {
    let (pipeline, api) = test_pipeline().await?;
    // The companion to the test above, with the sizes flipped: small pairs
    // replaced by large summaries, so the replacement grows the visible
    // conversation. The baseline (15k against a ~1.5k conversation) stands
    // for a request whose bulk was system prompt and tool schemas —
    // overhead the recount cannot see, counting messages alone. Standing
    // on the visible conversation (13k, under the 19.2k threshold) would
    // skip the compaction the real next request needs; subtracting the
    // hidden pairs and keeping the baseline crosses it. gpt-4.1's ~1M
    // canonical limit keeps the API's rejection wall far above every
    // request.
    let pipeline = pipeline
        .with_model_config(
            goose_providers::model::ModelConfig::new("gpt-4.1").with_context_limit(Some(24_000)),
        )
        .await;

    let mut history = vec![Message::user().with_text("run the tools")];
    let pairs = compute_tool_call_cutoff(pipeline.context_limit(), pipeline::COMPACTION_THRESHOLD)
        + TOOLCALL_SUMMARIZATION_BATCH_SIZE
        + 1;
    for i in 0..pairs as u32 {
        history.push(
            Message::assistant()
                .with_id(format!("req-{i}"))
                .with_tool_request(
                    format!("call-{i}"),
                    Ok(CallToolRequestParams::new(ADD).with_arguments(serde_json::Map::new())),
                ),
        );
        history.push(Message::user().with_tool_response(
            format!("call-{i}"),
            Ok(CallToolResult::success(vec![ContentBlock::text("1")])),
        ));
    }
    history.push(
        Message::assistant()
            .with_id("final")
            .with_text("all tools ran")
            .with_metadata(MessageMetadata {
                usage: Some(Box::new(MessageUsage::default())),
                ..MessageMetadata::default()
            }),
    );
    pipeline.seed_messages(history).await?;
    pipeline.set_total_tokens(15_000).await;

    // Ten summaries of ~1,250 tokens each replace ten ~40-token pairs: the
    // baseline minus the pairs plus the summaries reaches ~27k, over the
    // 19.2k threshold, while the visible conversation alone stays at ~13k.
    // The "carry on" rule covers the no-compaction regression path, where
    // the run ends on the plain kickoff inference instead.
    api.on("carry on").reply("carried on");
    api.on_system(SUMMARIZE_TOOL_PAIR).reply("x ".repeat(1_250));
    api.on(SUMMARIZE_HISTORY).reply("history summarized");
    api.on("Your context was compacted")
        .reply("compacted anyway");

    let run = pipeline.run(["carry on"]).await?;

    run.assert_message(-1, Agent, "compacted anyway");
    run.assert_emitted("Performing auto-compaction");
    assert_eq!(
        run.history_replacements(),
        1,
        "the baseline's overhead must survive the replacement"
    );
    assert_eq!(
        api.calls()
            .into_iter()
            .filter(|call| call.input_contains(SUMMARIZE_HISTORY))
            .count(),
        1,
        "the threshold crossing must trigger exactly one compaction"
    );
    assert_eq!(api.context_limit_rejections(), 0);

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

#[tokio::test]
async fn a_large_tool_result_alone_triggers_the_mid_turn_check() -> Result<()> {
    let (pipeline, api) = test_pipeline().await?;
    // Six parallel shell calls each land a ~10k-character (≈5k-token)
    // truncated preview, so the conversation grows by ~31k tokens in one
    // step — far past the 19.2k threshold — while the request that produced
    // the calls was under it. Only a mid-turn check that recounts the
    // conversation, rather than trusting the preceding request's usage, can
    // see the turn cross the threshold. gpt-4.1's ~1M canonical limit keeps
    // the API's rejection wall far above every request, summarization
    // included.
    let pipeline = pipeline
        .with_model_config(
            goose_providers::model::ModelConfig::new("gpt-4.1").with_context_limit(Some(24_000)),
        )
        .await;
    pipeline
        .add_extension_with_tools("developer", &["shell"])
        .await?;
    pipeline.set_permission(
        "shell",
        crate::config::permission::PermissionLevel::AlwaysAllow,
    );

    let huge_output = json!({ "command": "awk 'BEGIN{for(i=0;i<60000;i++)printf \"x \"}'" });
    api.on("read the huge outputs").calls([
        ("big-1", "shell", huge_output.clone()),
        ("big-2", "shell", huge_output.clone()),
        ("big-3", "shell", huge_output.clone()),
        ("big-4", "shell", huge_output.clone()),
        ("big-5", "shell", huge_output.clone()),
        ("big-6", "shell", huge_output),
    ]);
    api.on(SUMMARIZE_HISTORY).reply("huge outputs summarized");
    api.on("Your context was compacted")
        .reply("continued after the huge outputs");

    let run = pipeline.run(["read the huge outputs"]).await?;

    run.assert_message(-1, Agent, "continued after the huge outputs");
    assert_eq!(run.history_replacements(), 1);
    let summarization = api
        .calls()
        .into_iter()
        .find(|call| call.input_contains(SUMMARIZE_HISTORY))
        .expect("summarization request");
    assert!(
        summarization.input_tokens() > 60_000,
        "the summarization request must carry the huge tool results, i.e. the check ran mid-turn"
    );
    assert_eq!(api.context_limit_rejections(), 0);

    Ok(())
}

#[tokio::test]
async fn a_tool_loop_that_crosses_the_threshold_compacts_mid_turn() -> Result<()> {
    let (pipeline, api) = test_pipeline().await?;

    // The kickoff starts well under the threshold (0.8 * the model's 128k
    // canonical limit), so the only check that can see this turn grow is one
    // that runs after a tool response lands. Padding rides in the assistant
    // reply beside each tool call, growing the context past the trigger after
    // a few round-trips but below the API's hard 128k rejection.
    let padding = |round: usize| format!("padding {round} {}", "x".repeat(24_000));

    api.on("grow the context")
        .reasoning("looping")
        .reply(padding(0))
        .call(ADD, value(1));
    for round in 1..=4 {
        api.on(format!("result: {round}"))
            .reasoning("looping")
            .reply(padding(round))
            .call(ADD, value(1));
    }
    api.on(SUMMARIZE_HISTORY).reply("summary of the tool loop");
    api.on("Your context was compacted").reply("loop finished");

    let run = pipeline.run(["grow the context"]).await?;

    run.assert_message(-1, Agent, "loop finished");
    assert_eq!(
        run.history_replacements(),
        1,
        "the tool loop should compact exactly once"
    );
    let summarization = api
        .calls()
        .into_iter()
        .find(|call| call.input_contains(SUMMARIZE_HISTORY))
        .expect("summarization request");
    assert!(
        summarization.system_contains("result:"),
        "summarization ran while tool output was still in context, i.e. mid-turn"
    );
    assert_eq!(
        api.context_limit_rejections(),
        0,
        "compaction must fire before the provider rejects an oversized request"
    );
    assert!(
        !run.conversation()
            .messages()
            .iter()
            .any(|message| message.error_kind() == Some(MessageErrorKind::ContextLengthExceeded)),
        "a reactive compaction would have left a context-length error behind"
    );

    Ok(())
}

#[tokio::test]
async fn a_context_owning_provider_loops_tools_without_compacting() -> Result<()> {
    let (pipeline, api) = pipeline::test_pipeline_with(ProviderFeatures {
        manages_own_context: true,
        ..ProviderFeatures::default()
    })
    .await?;

    api.on("grow the context").call(ADD, value(1));
    api.on("result: 1").call(ADD, value(1));
    api.on("result: 2").reply("loop finished");

    // Past the threshold for the whole run, turn boundary included.
    pipeline
        .set_total_tokens((pipeline.context_limit() as f64 * 0.9) as i32)
        .await;

    let run = pipeline.run(["grow the context"]).await?;

    run.assert_message(-1, Agent, "loop finished");
    assert_eq!(run.history_replacements(), 0);
    assert!(!api
        .calls()
        .iter()
        .any(|call| call.input_contains(SUMMARIZE_HISTORY)));
    assert_eq!(api.context_limit_rejections(), 0);

    Ok(())
}

#[tokio::test]
async fn a_tool_result_added_to_the_provider_baseline_crosses_the_threshold() -> Result<()> {
    let (pipeline, api) = test_pipeline().await?;
    // Three parallel shell calls land ~15k tokens of truncated output. The
    // recounted conversation (~15.5k) and the preceding request's usage (the
    // serialized request, whose system overhead the conversation count cannot
    // see) each stay under the 19.2k threshold, so trusting only the larger
    // of the two misses the crossing; the unsent tool growth must be added to
    // the provider baseline for the mid-turn check to see it. gpt-4.1's ~1M
    // canonical limit keeps the API's rejection wall far above every request.
    let pipeline = pipeline
        .with_model_config(
            goose_providers::model::ModelConfig::new("gpt-4.1").with_context_limit(Some(24_000)),
        )
        .await;
    pipeline
        .add_extension_with_tools("developer", &["shell"])
        .await?;
    pipeline.set_permission(
        "shell",
        crate::config::permission::PermissionLevel::AlwaysAllow,
    );

    let huge_output = json!({ "command": "awk 'BEGIN{for(i=0;i<60000;i++)printf \"x \"}'" });
    api.on("grow past the baseline").calls([
        ("big-1", "shell", huge_output.clone()),
        ("big-2", "shell", huge_output.clone()),
        ("big-3", "shell", huge_output),
    ]);
    api.on(SUMMARIZE_HISTORY)
        .reply("baseline growth summarized");
    api.on("Your context was compacted")
        .reply("continued past the baseline");

    let run = pipeline.run(["grow past the baseline"]).await?;

    let producing_request = api.calls().first().cloned().expect("producing request");
    assert!(
        producing_request.input_tokens() < 19_200,
        "the producing request must stay under the threshold, got {}",
        producing_request.input_tokens()
    );

    run.assert_message(-1, Agent, "continued past the baseline");
    assert_eq!(
        run.history_replacements(),
        1,
        "the unsent tool growth must be added to the provider baseline"
    );
    let summarization = api
        .calls()
        .into_iter()
        .find(|call| call.input_contains(SUMMARIZE_HISTORY))
        .expect("summarization request");
    assert!(
        summarization.system_contains("x x x"),
        "the summarization request must carry the tool results, i.e. the check ran mid-turn"
    );
    assert_eq!(api.context_limit_rejections(), 0);

    Ok(())
}

#[tokio::test]
async fn a_pending_parallel_sibling_blocks_the_mid_turn_compaction() -> Result<()> {
    let (pipeline, api) = test_pipeline().await?;
    // A parallel batch pairs `load_skill` — answered by the skill operation,
    // which runs ahead of tool execution — with an ordinary `shell` call.
    // The skill body is large enough that its response alone crosses the
    // 19.2k threshold once added to the producing request's baseline, so the
    // pass after the skill response is over the threshold while the shell
    // request is still unanswered: compacting then would replace the history
    // and discard the request before it executes. The check must wait for
    // the batch to complete — the legacy loop persists the whole batch
    // before its check — and compact only once every response has landed.
    let pipeline = pipeline
        .with_model_config(
            goose_providers::model::ModelConfig::new("gpt-4.1").with_context_limit(Some(24_000)),
        )
        .await;
    pipeline
        .add_extension_with_tools("developer", &["shell"])
        .await?;
    pipeline.set_permission(
        "shell",
        crate::config::permission::PermissionLevel::AlwaysAllow,
    );

    let skill_dir = pipeline.working_dir().join(".agents/skills/review");
    std::fs::create_dir_all(&skill_dir).expect("skill dir");
    std::fs::write(
        skill_dir.join("SKILL.md"),
        format!(
            "---\nname: review\ndescription: Review helper\n---\n{}\n",
            "x ".repeat(20_000)
        ),
    )
    .expect("skill file");

    api.on("run the batch").calls([
        ("skill-1", "load_skill", json!({ "name": "review" })),
        (
            "sibling-1",
            "shell",
            json!({ "command": "echo done-$(echo 99)" }),
        ),
    ]);
    api.on(SUMMARIZE_HISTORY).reply("batch summarized");
    api.on("Your context was compacted")
        .reply("done after the batch");

    let run = pipeline.run(["run the batch"]).await?;

    run.assert_message(-1, Agent, "done after the batch");
    assert_eq!(
        run.history_replacements(),
        1,
        "compaction must run once the batch is complete"
    );
    let shell_responses = run
        .conversation()
        .messages()
        .iter()
        .flat_map(|message| message.content.iter())
        .filter(|content| {
            matches!(
                content,
                crate::conversation::message::MessageContent::ToolResponse(response)
                    if response.id == "sibling-1"
                        && response.tool_result.as_ref().is_ok_and(|result| {
                            result
                                .content
                                .iter()
                                .any(|block| block.as_text().is_some_and(|text| text.text.contains("done-99")))
                        })
            )
        })
        .count();
    assert_eq!(
        shell_responses, 1,
        "the shell sibling must execute exactly once, not be compacted away"
    );
    let summarization = api
        .calls()
        .into_iter()
        .find(|call| call.input_contains(SUMMARIZE_HISTORY))
        .expect("summarization request");
    assert!(
        summarization.system_contains("done-99"),
        "compaction must wait for the sibling's output before summarizing"
    );
    assert_eq!(api.context_limit_rejections(), 0);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_steer_drained_before_the_check_still_recounts_tool_growth() -> Result<()> {
    let (pipeline, api) = test_pipeline().await?;
    // The steer operation runs before compaction and appends the steer as
    // the last User message, so the check that follows it sees a User
    // boundary. Three shell calls grow the context by ~15k tokens of unsent
    // output; the producing request stays under the 19.2k threshold, and so
    // does the recounted conversation alone — only the unsent growth added
    // to the provider baseline crosses it. A check that only recounts at a
    // Tool boundary sends the grown context on. The delayed calculator call
    // holds the tool step open so the steer is queued mid-execution.
    let pipeline = pipeline
        .with_model_config(
            goose_providers::model::ModelConfig::new("gpt-4.1").with_context_limit(Some(24_000)),
        )
        .await;
    pipeline
        .add_extension_with_tools("developer", &["shell"])
        .await?;
    pipeline.set_permission(
        "shell",
        crate::config::permission::PermissionLevel::AlwaysAllow,
    );

    let huge_output = json!({ "command": "awk 'BEGIN{for(i=0;i<60000;i++)printf \"x \"}'" });
    api.on("grow then steer").calls([
        ("big-1", "shell", huge_output.clone()),
        ("big-2", "shell", huge_output.clone()),
        ("big-3", "shell", huge_output.clone()),
        (
            "slow-1",
            ADD,
            super::calculator_extension::delayed_value(7, 1_500),
        ),
    ]);
    api.on(SUMMARIZE_HISTORY).reply("steered work summarized");
    api.on("Your context was compacted")
        .reply("continued after the steer");

    let run = pipeline.run(["grow then steer"]);
    let steer = async {
        while pipeline.tool_contexts().is_empty() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        pipeline
            .steer(Message::user().with_text("redirect the work"))
            .await;
    };
    let (result, ()) = tokio::join!(run, steer);
    let result = result?;

    let producing_request = api.calls().first().cloned().expect("producing request");
    assert!(
        producing_request.input_tokens() < 19_200,
        "the producing request must stay under the threshold, got {}",
        producing_request.input_tokens()
    );

    result.assert_message(-1, Agent, "continued after the steer");
    assert_eq!(
        result.history_replacements(),
        1,
        "the tool growth behind the steer must still be recounted"
    );
    assert_eq!(
        api.call_count(),
        3,
        "compaction must fire before the grown context is sent back to the provider"
    );
    let summarization = api
        .calls()
        .into_iter()
        .find(|call| call.input_contains(SUMMARIZE_HISTORY))
        .expect("summarization request");
    assert!(
        summarization.system_contains("redirect the work"),
        "the summarization request must carry the drained steer"
    );
    assert!(!pipeline.has_pending_steers().await);
    assert_eq!(api.context_limit_rejections(), 0);

    Ok(())
}

#[tokio::test]
async fn a_failed_compaction_behind_a_steer_continues_the_run() -> Result<()> {
    let (pipeline, api) = test_pipeline().await?;
    // A steer drained after a tool round-trip reaches the check as a User
    // boundary, and the grown context crosses the 19.2k threshold — but the
    // summarization request fails. Ending the run there would drop the
    // drained steer, where the legacy loop's steer site logs the failure and
    // proceeds; the run must continue to the next inference and let the
    // reactive path own any oversized request. The delayed calculator call
    // holds the tool step open so the steer is queued mid-execution. Rules
    // match newest-first, so the summarization failure is registered last
    // while the recovery reply only matches a request that already carries
    // the tool results.
    let pipeline = pipeline
        .with_model_config(
            goose_providers::model::ModelConfig::new("gpt-4.1").with_context_limit(Some(24_000)),
        )
        .await;
    pipeline
        .add_extension_with_tools("developer", &["shell"])
        .await?;
    pipeline.set_permission(
        "shell",
        crate::config::permission::PermissionLevel::AlwaysAllow,
    );

    let huge_output = json!({ "command": "awk 'BEGIN{for(i=0;i<60000;i++)printf \"x \"}'" });
    api.on("grow then steer and fail").calls([
        ("big-1", "shell", huge_output.clone()),
        ("big-2", "shell", huge_output.clone()),
        ("big-3", "shell", huge_output),
        (
            "slow-1",
            ADD,
            super::calculator_extension::delayed_value(7, 1_500),
        ),
    ]);
    api.on("x x x x").reply("recovered behind the steer");
    api.on(SUMMARIZE_HISTORY).server_error("summarizer offline");

    let run = pipeline.run(["grow then steer and fail"]);
    let steer = async {
        while pipeline.tool_contexts().is_empty() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        pipeline
            .steer(Message::user().with_text("redirect the work"))
            .await;
    };
    let (result, ()) = tokio::join!(run, steer);
    let result = result?;

    result.assert_message(-1, Agent, "recovered behind the steer");
    assert_eq!(
        result.history_replacements(),
        0,
        "the failed compaction must not replace the history"
    );
    assert!(api
        .calls()
        .iter()
        .any(|call| call.input_contains(SUMMARIZE_HISTORY)));
    assert!(result
        .conversation()
        .messages()
        .iter()
        .all(|message| !message
            .as_concat_text()
            .contains("Ran into this error trying to compact")));
    assert_eq!(api.context_limit_rejections(), 0);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_compaction_behind_a_goal_nudge_continues_the_run() -> Result<()> {
    // Calibrate the serialized size of the kickoff request on a throwaway
    // session, so the goal can be sized from the real budget on any machine.
    let (calibration, api) = test_pipeline().await?;
    let calibration = calibration
        .with_model_config(
            goose_providers::model::ModelConfig::new("gpt-4.1").with_context_limit(Some(24_000)),
        )
        .await;
    api.on("calibrate the budget").reply("probe acknowledged");
    calibration.run(["calibrate the budget"]).await?;
    let first = api.calls().first().cloned().expect("calibration request");
    let threshold = (calibration.context_limit() as f64 * pipeline::COMPACTION_THRESHOLD) as i32;
    let target = threshold - first.input_tokens() + 1_000;
    assert!(
        target > 5_000,
        "kickoff request unexpectedly large: {}",
        first.input_tokens()
    );

    let (pipeline, api) = test_pipeline().await?;
    // A goal nudge is an agent-only user continuation: the text-only reply
    // ends the turn, the retry operation appends the nudge, and the next
    // pass reaches the compaction check over the 19.2k threshold. The nudge
    // boundary is neither mid-turn nor a steer, but the summarization
    // failure must still not end the run — the legacy loop appends the same
    // nudge after its checks and proceeds straight to inference, so the run
    // continues and the reactive path owns any oversized request. The goal
    // is sized in tokens — grown until the counter the check uses reports
    // enough to cross — because the baseline is reported in serialized
    // characters. Rules match newest-first, so the continuation rule
    // registered last wins even though the follow-up request also carries
    // the kickoff.
    let pipeline = pipeline
        .with_model_config(
            goose_providers::model::ModelConfig::new("gpt-4.1").with_context_limit(Some(24_000)),
        )
        .await;
    let counter = crate::token_counter::create_token_counter()
        .await
        .expect("token counter");
    let mut pairs = target.max(1) as usize;
    let goal = loop {
        let candidate =
            Message::user().with_text(format!("ship the release {}", "x ".repeat(pairs)));
        let tokens = counter.count_chat_tokens("", std::slice::from_ref(&candidate), &[]);
        if tokens as i32 >= target {
            break candidate.as_concat_text();
        }
        pairs += (target.saturating_sub(tokens as i32)).max(1) as usize;
    };
    pipeline.set_goal(Some(goal)).await;

    api.on("wrap up the work").reply("a text-only wrap-up");
    api.on("check whether the following goal")
        .reply("goal satisfied, finishing");
    api.on(SUMMARIZE_HISTORY).server_error("summarizer offline");

    let run = pipeline.run(["wrap up the work"]).await?;

    run.assert_message(-1, Agent, "goal satisfied, finishing");
    assert_eq!(
        run.history_replacements(),
        0,
        "the failed compaction must not replace the history"
    );
    assert!(api
        .calls()
        .iter()
        .any(|call| call.input_contains(SUMMARIZE_HISTORY)));
    assert!(run.conversation().messages().iter().all(|message| !message
        .as_concat_text()
        .contains("Ran into this error trying to compact")));
    assert_eq!(api.context_limit_rejections(), 0);
    assert_eq!(
        pipeline.get_goal().await,
        None,
        "the satisfied goal must be cleared"
    );

    Ok(())
}

#[tokio::test]
async fn a_failed_mid_turn_compaction_continues_the_turn() -> Result<()> {
    let (pipeline, api) = test_pipeline().await?;
    // Three shell calls grow the context past the 19.2k threshold, but the
    // summarization request fails. The pass must continue to the next
    // inference instead of yielding the compaction error: the request that
    // follows may still fit, and the reactive path owns the outcome if it
    // does not. Rules match newest-first, so the summarization failure is
    // registered last while the recovery reply only matches a request that
    // already carries the tool results.
    let pipeline = pipeline
        .with_model_config(
            goose_providers::model::ModelConfig::new("gpt-4.1").with_context_limit(Some(24_000)),
        )
        .await;
    pipeline
        .add_extension_with_tools("developer", &["shell"])
        .await?;
    pipeline.set_permission(
        "shell",
        crate::config::permission::PermissionLevel::AlwaysAllow,
    );

    let huge_output = json!({ "command": "awk 'BEGIN{for(i=0;i<60000;i++)printf \"x \"}'" });
    api.on("recover from a failed compaction").calls([
        ("big-1", "shell", huge_output.clone()),
        ("big-2", "shell", huge_output.clone()),
        ("big-3", "shell", huge_output),
    ]);
    api.on("x x x x").reply("recovered without compaction");
    api.on(SUMMARIZE_HISTORY).server_error("summarizer offline");

    let run = pipeline.run(["recover from a failed compaction"]).await?;

    run.assert_message(-1, Agent, "recovered without compaction");
    assert_eq!(
        run.history_replacements(),
        0,
        "the failed compaction must not replace the history"
    );
    assert!(api
        .calls()
        .iter()
        .any(|call| call.input_contains(SUMMARIZE_HISTORY)));
    assert!(run.conversation().messages().iter().all(|message| !message
        .as_concat_text()
        .contains("Ran into this error trying to compact")));
    assert_eq!(api.context_limit_rejections(), 0);

    Ok(())
}

#[tokio::test]
async fn a_stale_token_count_floors_on_the_full_conversation() -> Result<()> {
    let (pipeline, api) = test_pipeline().await?;
    // The session metadata reports 1,000 tokens while the seeded history
    // holds far more against the 19.2k threshold, and the bulk of it sits
    // before the newest assistant message. Only a recount whose total covers
    // the whole conversation can floor the stale metadata; one that stopped
    // counting at the newest assistant message would send the oversized
    // history on. gpt-4.1's ~1M canonical limit keeps the API's rejection
    // wall far above every request.
    let pipeline = pipeline
        .with_model_config(
            goose_providers::model::ModelConfig::new("gpt-4.1").with_context_limit(Some(24_000)),
        )
        .await;
    pipeline
        .seed([
            Message::user().with_text("x ".repeat(22_000)),
            Message::assistant().with_text("done"),
        ])
        .await?;
    pipeline.set_total_tokens(1_000).await;

    api.on(SUMMARIZE_HISTORY).reply("stale history summarized");
    api.on("Your context was compacted")
        .reply("continued past the stale count");

    let run = pipeline.run(["continue the stale session"]).await?;

    run.assert_message(-1, Agent, "continued past the stale count");
    assert_eq!(
        run.history_replacements(),
        1,
        "the recounted conversation must floor the stale metadata"
    );
    assert_eq!(
        api.call_count(),
        2,
        "compaction must precede the first inference"
    );
    assert_eq!(api.context_limit_rejections(), 0);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_large_text_only_reply_alone_does_not_compact_behind_a_steer() -> Result<()> {
    // Calibrate the serialized size of the kickoff request on a throwaway
    // session, so the dense reply can be sized from the real budget on any
    // machine.
    let (calibration, api) = test_pipeline().await?;
    let calibration = calibration
        .with_model_config(
            goose_providers::model::ModelConfig::new("gpt-4.1").with_context_limit(Some(24_000)),
        )
        .await;
    api.on("answer densely without tools")
        .reply("probe acknowledged");
    calibration.run(["answer densely without tools"]).await?;
    let first = api.calls().first().cloned().expect("calibration request");
    let threshold = (calibration.context_limit() as f64 * pipeline::COMPACTION_THRESHOLD) as i32;
    let budget = threshold - first.input_tokens();
    assert!(
        budget > 5_000,
        "kickoff request unexpectedly large: {}",
        first.input_tokens()
    );

    let (pipeline, api) = test_pipeline().await?;
    let pipeline = pipeline
        .with_model_config(
            goose_providers::model::ModelConfig::new("gpt-4.1").with_context_limit(Some(24_000)),
        )
        .await;
    // The reply is text-only and dense — CJK text measures roughly one token
    // per character — so its reported output (the dummy API bills by
    // serialized character) and its recounted tokens are nearly the same
    // size. The reply is sized to leave the baseline under the 19.2k
    // threshold once the small steer lands: the real next request — the
    // baseline plus the steer — fits, so no compaction may run. A recount
    // that also counted the reply itself as unsent growth would add its
    // tokens on top of a baseline that already contains them as output and
    // summarize prematurely.
    let reply = "你".repeat((budget as f64 * 0.6) as usize);
    api.on("answer densely without tools").reply(reply);
    api.on("and keep going").reply("followed the small steer");
    api.on(SUMMARIZE_HISTORY).reply("prematurely summarized");
    api.on("Your context was compacted")
        .reply("compacted anyway");
    // The queued steer waits for the reply's turn boundary, so it lands
    // between the reply and the check that follows it.
    pipeline
        .steer(Message::user().with_text("and keep going"))
        .await;

    let result = pipeline.run(["answer densely without tools"]).await?;

    result.assert_message(-1, Agent, "followed the small steer");
    assert_eq!(
        result.history_replacements(),
        0,
        "a reply the baseline already counts must not be added to it as growth"
    );
    assert!(
        !api.calls()
            .iter()
            .any(|call| call.input_contains(SUMMARIZE_HISTORY)),
        "no summarization may run while the real next request still fits"
    );
    assert!(!pipeline.has_pending_steers().await);
    assert_eq!(api.context_limit_rejections(), 0);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_large_steer_after_a_text_only_reply_rechecks_the_threshold() -> Result<()> {
    let (pipeline, api) = test_pipeline().await?;
    // The first reply is text-only and stays under the 19.2k threshold, so
    // the steers are what push past it: the check that runs after the steer
    // operation must recount and compact before the grown context is sent
    // back. The large steer is sized from the first request's reported usage
    // so the crossing comes from the unsent growth added to the baseline,
    // and the small steer behind it is what compaction preserves, keeping
    // the retained context far under the threshold.
    let pipeline = pipeline
        .with_model_config(
            goose_providers::model::ModelConfig::new("gpt-4.1").with_context_limit(Some(24_000)),
        )
        .await;

    let text_only = api
        .on("reply without tools")
        .hold_reply("a text-only answer");
    api.on(SUMMARIZE_HISTORY).reply("steer growth summarized");
    api.on("Your context was compacted")
        .reply("continued after the steer");

    let run = pipeline.run(["reply without tools"]);
    let steer = async {
        text_only.entered().await;
        let first = api.calls().first().cloned().expect("text-only request");
        let threshold = (pipeline.context_limit() as f64 * pipeline::COMPACTION_THRESHOLD) as i32;
        // The recount adds each drained steer's measured tokens to the
        // reported baseline, so the growth must be sized in tokens, not
        // characters: the baseline is reported in serialized characters
        // while the growth is counted in tokens, and machines whose first
        // request differs can leave a character-sized steer short of the
        // crossing. Grow the first steer until the counter — the same one
        // the check uses — reports enough tokens to cross with ~1k to
        // spare. A second, small steer rides behind it: compaction
        // preserves the most recent text-only user message verbatim, so the
        // large one is summarized away and the retained context cannot
        // re-trigger compaction no matter how large the first steer had to
        // grow.
        let target = threshold - first.input_tokens() + 1_000;
        assert!(
            target > 5_000,
            "first request unexpectedly large: {}",
            first.input_tokens()
        );
        let counter = crate::token_counter::create_token_counter()
            .await
            .expect("token counter");
        let mut pairs = target.max(1) as usize;
        let large = loop {
            let candidate =
                Message::user().with_text(format!("redirect the work {}", "x ".repeat(pairs)));
            let tokens = counter.count_chat_tokens("", std::slice::from_ref(&candidate), &[]);
            if tokens as i32 >= target {
                break candidate;
            }
            pairs += (target.saturating_sub(tokens as i32)).max(1) as usize;
        };
        pipeline.steer(large).await;
        pipeline
            .steer(Message::user().with_text("and keep it brief"))
            .await;
        text_only.release();
    };
    let (result, ()) = tokio::join!(run, steer);
    let result = result?;

    result.assert_message(-1, Agent, "continued after the steer");
    assert_eq!(
        result.history_replacements(),
        1,
        "the drained steer must be re-checked against the threshold"
    );
    assert_eq!(
        api.call_count(),
        3,
        "compaction must fire before the grown context is sent back to the provider"
    );
    let summarization = api
        .calls()
        .into_iter()
        .find(|call| call.input_contains(SUMMARIZE_HISTORY))
        .expect("summarization request");
    assert!(
        summarization.system_contains("redirect the work"),
        "the summarization request must carry the drained steer"
    );
    assert_eq!(api.context_limit_rejections(), 0);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_kickoff_preserved_by_compaction_is_not_recompacted() -> Result<()> {
    let (pipeline, api) = test_pipeline().await?;
    // The replacement installs a session baseline that already covers the
    // whole compacted history — summary plus the kickoff preserved verbatim —
    // but that history has no usage marker, so a recount would fall back to
    // the summary boundary and count the preserved kickoff as unsent growth
    // on top of a baseline that already contains it. The kickoff is sized so
    // the retained history stays under the 192k threshold while counting the
    // kickoff twice crosses it: without a boundary the re-check after
    // replacement re-compacts on every pass and never reaches inference.
    let pipeline = pipeline
        .with_model_config(
            goose_providers::model::ModelConfig::new("gpt-4.1").with_context_limit(Some(240_000)),
        )
        .await;
    // The kickoff's serialized size approaches the threshold while the
    // compaction check measures tokens, so pin the API's rejection wall far
    // above everything: only the operation's own threshold may stop a pass.
    api.set_context_limit(2_000_000);
    api.on("reply briefly").reply("acknowledged");
    api.on("grow the context").reply("grew");
    api.on(SUMMARIZE_HISTORY).reply("history summarized");
    api.on("Your context was compacted")
        .reply("continued after compaction");

    pipeline.run(["reply briefly"]).await?;
    // Lift the baseline so the large kickoff crosses the threshold when it
    // lands: the recount adds its measured tokens to the baseline.
    pipeline.set_total_tokens(95_000).await;

    let threshold = (pipeline.context_limit() as f64 * pipeline::COMPACTION_THRESHOLD) as i32;
    // Size the kickoff in tokens — the same counter the check uses — to a
    // bit past half the threshold: the retained history (kickoff plus a
    // small summary) stays under it while the kickoff counted twice crosses
    // it on any machine.
    let target = (threshold as f64 * 0.55) as usize;
    let counter = crate::token_counter::create_token_counter()
        .await
        .expect("token counter");
    let mut pairs = target;
    let kickoff = loop {
        let candidate = format!("grow the context {}", "x ".repeat(pairs));
        let message = Message::user().with_text(&candidate);
        let tokens = counter.count_chat_tokens("", std::slice::from_ref(&message), &[]);
        if tokens >= target {
            break candidate;
        }
        pairs += (target.saturating_sub(tokens)).max(1);
    };

    let run = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        pipeline.run([kickoff.as_str()]),
    )
    .await
    .expect("the run must reach inference instead of re-compacting every pass")?;

    run.assert_message(-1, Agent, "continued after compaction");
    run.assert_emitted("Performing auto-compaction");
    assert_eq!(
        run.history_replacements(),
        1,
        "the replaced kickoff is already counted by the replacement baseline"
    );
    assert_eq!(
        api.calls()
            .iter()
            .filter(|call| call.input_contains(SUMMARIZE_HISTORY))
            .count(),
        1,
        "the preserved kickoff must not be re-added to the baseline as unsent growth"
    );
    assert_eq!(api.context_limit_rejections(), 0);

    Ok(())
}
