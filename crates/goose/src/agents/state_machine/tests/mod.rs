use anyhow::Result;

use self::calculator_extension::{value, ADD};
use self::pipeline::MessageKind::{Agent, ToolCall};
use self::pipeline::{test_pipeline, MAX_TURNS};
use crate::agents::state_machine;
use crate::agents::state_machine::ops_retry::NUDGED;
use crate::agents::state_machine::Emitter;

mod agent_reply;
mod calculator_extension;
mod compaction_lifecycle;
mod dummy_api;
mod hooks_lifecycle;
mod pipeline;
mod prompt_skill_lifecycle;
mod provider_lifecycle;
mod recipe_scheduling_lifecycle;
mod reconstruction_isolation_lifecycle;
mod steering_lifecycle;
mod tool_lifecycle;

async fn capture_state_machine_trace_fields(
    capture_setting: Option<&'static str>,
) -> Result<serde_json::Map<String, serde_json::Value>> {
    use goose_test_support::otel::clear_otel_env;
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;
    use tracing_futures::Instrument;

    use crate::agents::gen_ai_telemetry::{
        test_support::SpanFieldCapture, CAPTURE_MESSAGE_CONTENT_ENV,
    };
    use crate::conversation::message::Message;

    let _env = match capture_setting {
        Some(value) => clear_otel_env(&[(CAPTURE_MESSAGE_CONTENT_ENV, value)]),
        None => clear_otel_env(&[]),
    };
    let capture = SpanFieldCapture::new("state_machine_security_trace");
    let _subscriber = capture.clone().set_default();
    let (pipeline, api) = test_pipeline().await?;
    api.on("input-super-secret-token")
        .reply("output-super-secret-token");
    pipeline
        .session_manager
        .add_message(
            &pipeline.session_id,
            &Message::user().with_text("input-super-secret-token"),
        )
        .await?;

    let cancel = CancellationToken::new();
    let machine = pipeline.machine(cancel.clone());
    let (tx, _rx) = mpsc::channel(1024);
    let emit = Emitter::new(tx, cancel);
    let span = tracing::info_span!(
        "state_machine_security_trace",
        trace_input = tracing::field::Empty,
        trace_output = tracing::field::Empty,
        gen_ai.agent.name = tracing::field::Empty,
        gen_ai.output.messages = tracing::field::Empty,
    );
    super::session::run(
        &machine,
        pipeline.session_manager.as_ref(),
        &pipeline.session_id,
        &emit,
    )
    .instrument(span)
    .await?;

    Ok(capture.fields())
}

#[tokio::test]
async fn state_machine_trace_omits_content_without_capture() -> Result<()> {
    for capture_setting in [None, Some("false")] {
        let fields = capture_state_machine_trace_fields(capture_setting).await?;
        let recorded = serde_json::to_string(&fields)?;
        assert!(!recorded.contains("super-secret-token"));
        assert!(!fields.contains_key("trace_input"));
        assert!(!fields.contains_key("trace_output"));
        assert!(!fields.contains_key("gen_ai.output.messages"));
        assert_eq!(fields["gen_ai.agent.name"], "goose");
    }
    Ok(())
}

#[tokio::test]
async fn state_machine_trace_retains_content_with_capture() -> Result<()> {
    let fields = capture_state_machine_trace_fields(Some("true")).await?;
    assert_eq!(fields["trace_input"], "input-super-secret-token");
    assert_eq!(fields["trace_output"], "output-super-secret-token");
    assert!(fields["gen_ai.output.messages"]
        .as_str()
        .unwrap()
        .contains("output-super-secret-token"));
    Ok(())
}

#[tokio::test]
async fn bang_shell_requests_the_shell_tool() -> Result<()> {
    let (pipeline, api) = test_pipeline().await?;

    let result = pipeline.run(["!echo hello"]).await?;
    result.assert_message(1, ToolCall, r#"shell({"command":"echo hello"})"#);
    assert_eq!(api.call_count(), 0);

    Ok(())
}

#[tokio::test]
async fn max_turns_counts_inference_calls_and_injects_budget() -> Result<()> {
    let (pipeline, api) = test_pipeline().await?;
    api.on("keep going").call(ADD, value(1));

    let result = pipeline.run(["keep going"]).await?;
    let calls = api.calls();
    assert_eq!(calls.len(), MAX_TURNS as usize);
    assert_eq!(pipeline.calculator_total(), MAX_TURNS as i64 - 1);

    let first_budgeted_call = MAX_TURNS.div_ceil(2) as usize;
    assert!(!calls[first_budgeted_call - 1].input_contains("<turn-budget>"));
    assert!(calls[first_budgeted_call].input_contains("<turn-budget>"));
    result.assert_message(-1, Agent, state_machine::MAX_TURNS_MESSAGE);
    result.assert_emitted_message_matches_persisted(state_machine::MAX_TURNS_MESSAGE);

    Ok(())
}

#[tokio::test]
async fn turn_context_is_persisted_once_per_turn_and_reused_across_inferences() -> Result<()> {
    let (pipeline, api) = test_pipeline().await?;
    api.on("add one").call(ADD, value(1));
    api.on("result: 1").reply("The total is 1");
    api.on("hello").reply("hi there!");

    let result = pipeline.run(["add one", "hello"]).await?;

    let conversation = result.conversation();
    let events: Vec<_> = conversation
        .messages()
        .iter()
        .filter(|message| message.is_turn_context())
        .collect();
    assert_eq!(
        events.len(),
        2,
        "one turn-context event per turn; the turn's second inference reuses it"
    );
    assert!(events.iter().all(|event| !event.is_user_visible()));
    assert!(api
        .calls()
        .iter()
        .all(|call| call.input_contains("<turn-context>")));

    Ok(())
}

#[tokio::test]
async fn goal_starts_nudges_and_clears_when_met() -> Result<()> {
    let (pipeline, api) = test_pipeline().await?;
    api.on("Start working toward this goal now")
        .reply("did some work");
    api.on("fully met").reply("goal is met");

    let (pipeline, result, _) = pipeline
        .run_reconstructing_each_step("/goal finish the migration")
        .await?;

    assert_eq!(api.call_count(), 2);
    assert!(pipeline.get_goal().await.is_none());
    result.assert_message(-1, Agent, "goal is met");

    let command = result
        .conversation()
        .messages()
        .iter()
        .find(|message| message.as_concat_text() == "/goal finish the migration")
        .expect("persisted goal command");
    assert!(command.is_user_visible());
    assert!(!command.is_agent_visible());

    result
        .conversation()
        .messages()
        .iter()
        .find(|message| {
            message.as_concat_text().contains("finish the migration")
                && !message.is_user_visible()
                && message.is_agent_visible()
        })
        .expect("hidden goal kickoff");
    result
        .conversation()
        .messages()
        .iter()
        .find(|message| {
            message.as_concat_text().contains("fully met")
                && !message.is_user_visible()
                && message.is_agent_visible()
        })
        .expect("hidden goal nudge");
    assert_eq!(
        result
            .conversation()
            .messages()
            .iter()
            .filter(|message| message.metadata.operation_note("retry", NUDGED).is_some())
            .count(),
        1
    );

    Ok(())
}

#[tokio::test]
async fn grind_is_bounded_by_max_turns() -> Result<()> {
    let (pipeline, api) = test_pipeline().await?;
    api.on("go").reply("grinding");
    api.on("never done").reply("grinding");
    pipeline.set_grind(Some("never done".to_string())).await;

    let result = pipeline.run(["go"]).await?;

    assert_eq!(api.call_count(), MAX_TURNS as usize);
    result.assert_message(-1, Agent, state_machine::MAX_TURNS_MESSAGE);

    Ok(())
}

#[tokio::test]
async fn slash_commands_yield_or_fall_through_to_inference() -> Result<()> {
    let (pipeline, api) = test_pipeline().await?;

    let status = pipeline.run(["/status"]).await?;
    assert_eq!(api.call_count(), 0);
    status.assert_message(-1, Agent, "Provider:");
    assert!(status
        .conversation()
        .messages()
        .iter()
        .all(|message| message.is_user_visible() && !message.is_agent_visible()));

    api.on("/not-a-command").reply("saw it");
    let unknown = pipeline.run(["/not-a-command"]).await?;
    assert_eq!(api.call_count(), 1);
    unknown.assert_message(-1, Agent, "saw it");
    let command = unknown
        .conversation()
        .messages()
        .iter()
        .find(|message| message.as_concat_text() == "/not-a-command")
        .expect("persisted user message");
    assert!(command.is_user_visible());
    assert!(command.is_agent_visible());

    Ok(())
}
