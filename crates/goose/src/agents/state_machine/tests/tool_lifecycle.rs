use anyhow::Result;
use rmcp::model::ElicitationAction;
use serde_json::json;

use super::calculator_extension::{
    delayed_value, value, ADD, ADD_WITH_AUDIENCE, APP_ONLY, DIVIDE, REQUEST_VALUE,
};
use super::pipeline::MessageKind::{Agent, Confirmation, ToolCall, ToolResponse};
use super::pipeline::MAX_TURNS;
use super::test_pipeline;
use crate::agents::tool_execution::{CHAT_MODE_TOOL_SKIPPED_RESPONSE, DECLINED_RESPONSE};
use crate::agents::AgentEvent;
use crate::config::permission::PermissionLevel;
use crate::config::GooseMode;
use crate::conversation::message::{Message, MessageContent};
use crate::permission::Permission;

#[tokio::test]
async fn basic_tool_calling() -> Result<()> {
    let (pipeline, api) = test_pipeline().await?;
    api.on("add one").call(ADD, value(1));
    api.on("result: 1").reply("The total is 1");
    api.on("hello").reply("hi there!");

    let result = pipeline.run(["add one", "hello"]).await?;

    result.assert_message(2, ToolResponse, "");
    result.assert_message(3, Agent, "The total is 1");
    result.assert_message(-1, Agent, "hi there!");
    assert_eq!(api.call_count(), 3);
    Ok(())
}

#[tokio::test]
async fn provider_turn_executes_only_advertised_registered_tools() -> Result<()> {
    let (pipeline, api) = test_pipeline().await?;
    api.on("try the app-only tool")
        .unadvertised_call(APP_ONLY, value(10));
    api.on("not available").reply("app-only blocked");

    let result = pipeline.run(["try the app-only tool"]).await?;

    assert!(!api.calls()[0].advertises_tool(APP_ONLY));
    result.assert_message(-2, ToolResponse, "not available");
    result.assert_message(-1, Agent, "app-only blocked");
    assert_eq!(pipeline.calculator_total(), 0);

    api.on("use an advertised tool").call(ADD, value(1));
    api.on("result: 1").reply("advertised tool ran");

    let result = pipeline.run(["use an advertised tool"]).await?;

    result.assert_message(-2, ToolResponse, "result: 1");
    result.assert_message(-1, Agent, "advertised tool ran");
    assert_eq!(pipeline.calculator_total(), 1);

    Ok(())
}

#[tokio::test]
async fn provider_turn_canonicalizes_only_advertised_mangled_tool_names() -> Result<()> {
    let (pipeline, api) = test_pipeline().await?;
    api.on("use a mangled advertised tool")
        .unadvertised_call("calculator.add", value(1));
    api.on("result: 1").reply("mangled tool ran");

    let result = pipeline.run(["use a mangled advertised tool"]).await?;

    result.assert_message(-3, ToolCall, ADD);
    result.assert_message(-2, ToolResponse, "result: 1");
    result.assert_message(-1, Agent, "mangled tool ran");
    assert_eq!(pipeline.calculator_total(), 1);

    api.on("try a mangled app-only tool")
        .unadvertised_call("calculator.app_only", value(10));
    api.on("not available").reply("mangled app-only blocked");

    let result = pipeline.run(["try a mangled app-only tool"]).await?;

    result.assert_message(-2, ToolResponse, "not available");
    result.assert_message(-1, Agent, "mangled app-only blocked");
    assert_eq!(pipeline.calculator_total(), 1);

    Ok(())
}

#[tokio::test]
async fn toolshim_provider_turn_executes_advertised_tools() -> Result<()> {
    let (pipeline, api) = test_pipeline().await?;
    let model_config =
        goose_providers::model::ModelConfig::new(goose_providers::openai::OPEN_AI_DEFAULT_MODEL)
            .with_canonical_limits("openai")
            .with_toolshim(true);
    let pipeline = pipeline.with_model_config(model_config).await;
    api.on("use an advertised tool through toolshim")
        .unadvertised_call(ADD, value(2));
    api.on("result: 2").reply("toolshim tool ran");

    let result = pipeline
        .run(["use an advertised tool through toolshim"])
        .await?;

    result.assert_message(-2, ToolResponse, "result: 2");
    result.assert_message(-1, Agent, "toolshim tool ran");
    assert_eq!(pipeline.calculator_total(), 2);

    Ok(())
}

#[tokio::test]
async fn recover_from_faulty_call() -> Result<()> {
    let (pipeline, api) = test_pipeline().await?;
    api.on("make a broken tool call")
        .malformed_tool_call(ADD, "");
    api.on("missing field").reply("The calculation failed");
    api.on("divide by zero").call(DIVIDE, value(0));
    api.on("calculator operation failed")
        .reply("The calculator failed");
    api.on("add after the failures").call(ADD, value(1));
    api.on("result: 1").reply("still here");

    let result = pipeline
        .run([
            "make a broken tool call",
            "divide by zero",
            "add after the failures",
        ])
        .await?;

    result.assert_message(2, ToolResponse, "missing field");
    result.assert_message(6, ToolResponse, "calculator operation failed");
    result.assert_message(10, ToolResponse, "result: 1");
    result.assert_message(-1, Agent, "still here");
    assert_eq!(pipeline.calculator_total(), 1);
    Ok(())
}

#[tokio::test]
async fn recover_from_missing_tool() -> Result<()> {
    let (pipeline, api) = test_pipeline().await?;
    api.on("try the missing tool")
        .unadvertised_call("missing__tool", json!({}));
    api.on("Available tools").call(ADD, value(2));
    api.on("result: 2").reply("recovered");

    let result = pipeline.run(["try the missing tool"]).await?;
    assert_eq!(api.call_count(), 3);

    result.assert_message(2, ToolResponse, "Tool 'missing__tool' is not available");
    assert!(api.calls()[1].input_contains(ADD));
    result.assert_message(3, ToolCall, ADD);
    result.assert_message(4, ToolResponse, "result: 2");
    result.assert_message(-1, Agent, "recovered");
    assert_eq!(pipeline.calculator_total(), 2);

    let (pipeline, api) = test_pipeline().await?;
    let model_config =
        goose_providers::model::ModelConfig::new(goose_providers::openai::OPEN_AI_DEFAULT_MODEL)
            .with_canonical_limits("openai")
            .with_toolshim(true);
    let pipeline = pipeline.with_model_config(model_config).await;
    api.on("try the missing tool in toolshim")
        .unadvertised_call("missing__tool", json!({}));
    api.on("Available tools").reply("recovered with toolshim");

    let result = pipeline.run(["try the missing tool in toolshim"]).await?;
    assert!(api.calls()[1].input_contains(ADD));
    result.assert_message(-1, Agent, "recovered with toolshim");

    Ok(())
}

#[tokio::test]
async fn malformed_tool_calls_recover_and_remain_bounded() -> Result<()> {
    let (pipeline, api) = test_pipeline().await?;
    api.on("do it").malformed_tool_call(ADD, r#"{"value":"#);
    api.on("could not be parsed").reply("let me fix that");

    let result = pipeline.run(["do it"]).await?;
    result.assert_message(2, ToolResponse, "could not be parsed");
    assert_eq!(api.call_count(), 2);
    result.assert_message(-1, Agent, "let me fix that");

    api.on("repeat malformed forever")
        .malformed_tool_call(ADD, r#"{"value":"#);
    let calls_before = api.call_count();
    let result = pipeline.run(["repeat malformed forever"]).await?;

    assert_eq!(api.call_count() - calls_before, MAX_TURNS as usize);
    result.assert_message(-1, Agent, crate::agents::state_machine::MAX_TURNS_MESSAGE);

    Ok(())
}

#[tokio::test]
async fn multiple_tool_calls_are_paired_and_executed_once() -> Result<()> {
    let (pipeline, api) = test_pipeline().await?;
    pipeline.synchronize_calculator(2);
    api.on("run several calculations").calls([
        ("first", ADD, delayed_value(1, 50)),
        ("duplicate", ADD, value(2)),
        ("duplicate", ADD, value(100)),
    ]);
    api.on("result: 3").reply("done");

    let result = pipeline.run(["run several calculations"]).await?;

    result.assert_message(1, ToolCall, ADD);
    result.assert_message(2, ToolCall, ADD);
    result.assert_message(3, ToolResponse, "result: 2");
    result.assert_message(4, ToolResponse, "result: 3");
    result.assert_message(-1, Agent, "done");
    assert_eq!(pipeline.calculator_total(), 3);

    let conversation = result.session.conversation.unwrap_or_default();
    let mut request_ids = conversation
        .messages()
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|content| match content {
            MessageContent::ToolRequest(request) => Some(request.id.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let mut response_ids = conversation
        .messages()
        .iter()
        .flat_map(Message::get_tool_response_ids)
        .collect::<Vec<_>>();
    request_ids.sort();
    response_ids.sort();
    assert_eq!(request_ids, ["duplicate", "first"]);
    assert_eq!(response_ids, request_ids);

    Ok(())
}

#[tokio::test]
async fn approvals_and_per_tool_permissions() -> Result<()> {
    let (pipeline, api) = test_pipeline().await?;
    let pipeline = pipeline.with_goose_mode(GooseMode::Approve).await;

    pipeline.synchronize_calculator(2);
    api.on("add one and two")
        .calls([("first", ADD, value(1)), ("second", ADD, value(2))]);
    api.on("result: 3").reply("both approved");

    let awaiting_confirmation = pipeline.run(["add one and two"]).await?;
    assert_eq!(pipeline.calculator_total(), 0);
    awaiting_confirmation.assert_message(1, ToolCall, ADD);
    awaiting_confirmation.assert_message(2, ToolCall, ADD);

    pipeline.confirm("second", Permission::AllowOnce).await?;
    pipeline.confirm("first", Permission::AllowOnce).await?;
    let result = pipeline.resume().await?;
    result.assert_message(-1, Agent, "both approved");
    assert_eq!(pipeline.calculator_total(), 3);

    api.on("deny this addition")
        .calls([("denied", ADD, value(10))]);
    api.on(DECLINED_RESPONSE).reply("I will not retry it");
    pipeline.run(["deny this addition"]).await?;
    pipeline.confirm("denied", Permission::DenyOnce).await?;
    let result = pipeline.resume().await?;
    result.assert_message(-2, ToolResponse, "DO NOT attempt to call this tool again");
    result.assert_message(-1, Agent, "I will not retry it");
    assert_eq!(pipeline.calculator_total(), 3);

    api.on("always deny division")
        .calls([("always-denied", DIVIDE, value(2))]);
    pipeline.run(["always deny division"]).await?;
    pipeline
        .confirm("always-denied", Permission::AlwaysDeny)
        .await?;
    api.on(DECLINED_RESPONSE).reply("division denied forever");
    let result = pipeline.resume().await?;
    result.assert_message(-1, Agent, "division denied forever");

    pipeline.set_permission(ADD, PermissionLevel::AlwaysAllow);
    api.on("apply the saved permissions")
        .calls([("allowed", ADD, value(1)), ("blocked", DIVIDE, value(2))]);
    api.on("result: 4").reply("permissions applied");
    let result = pipeline.run(["apply the saved permissions"]).await?;
    result.assert_message(-3, ToolResponse, DECLINED_RESPONSE);
    result.assert_message(-2, ToolResponse, "result: 4");
    result.assert_message(-1, Agent, "permissions applied");
    assert_eq!(pipeline.calculator_total(), 4);

    let pipeline = pipeline.with_goose_mode(GooseMode::Auto).await;
    pipeline.add_extension("developer").await?;
    api.on("run a dangerous command").calls([(
        "dangerous",
        "shell",
        json!({ "command": "rm -rf /" }),
    )]);

    let result = pipeline.run(["run a dangerous command"]).await?;
    result.assert_message(-1, Confirmation, "Security Alert");

    api.on(DECLINED_RESPONSE).reply("the command was not run");
    pipeline.confirm("dangerous", Permission::DenyOnce).await?;
    let result = pipeline.resume().await?;
    result.assert_message(-2, ToolResponse, DECLINED_RESPONSE);
    result.assert_message(-1, Agent, "the command was not run");

    Ok(())
}

#[tokio::test]
async fn execution_recovers_from_timeout_cancellation_and_filtered_output() -> Result<()> {
    let (pipeline, api) = test_pipeline().await?;

    api.on("time out").call(ADD, delayed_value(10, 150));
    api.on("calculator operation timed out")
        .reply("recovered from timeout");
    let result = pipeline.run(["time out"]).await?;
    result.assert_message(-2, ToolResponse, "calculator operation timed out");
    result.assert_message(-1, Agent, "recovered from timeout");
    assert_eq!(pipeline.calculator_total(), 0);

    api.on("cancel this").calls([
        ("completed", ADD, value(1)),
        ("cancelled", ADD, delayed_value(10, 500)),
    ]);
    let cancel = tokio_util::sync::CancellationToken::new();
    let run = pipeline.run_with_cancel("cancel this", cancel.clone());
    let cancel_after_result = async {
        pipeline.wait_for_calculator_result().await;
        cancel.cancel();
    };
    let (result, ()) = tokio::join!(run, cancel_after_result);
    let result = result?;
    result.assert_message(-2, ToolResponse, "result: 1");
    result.assert_message(-1, ToolResponse, "calculator call cancelled");
    assert_eq!(pipeline.calculator_total(), 1);

    api.on("continue after cancellation").call(ADD, value(1));
    api.on("result: 2").reply("continued");
    let result = pipeline.run(["continue after cancellation"]).await?;
    result.assert_message(-2, ToolResponse, "result: 2");
    result.assert_message(-1, Agent, "continued");

    api.on("show the result once")
        .call(ADD_WITH_AUDIENCE, value(1));
    api.on("result: 3").reply("shown once");
    let result = pipeline.run(["show the result once"]).await?;
    result.assert_message(-1, Agent, "shown once");
    let emitted_result_count = result
        .events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::Message(message) => Some(message),
            _ => None,
        })
        .flat_map(|message| &message.content)
        .filter_map(MessageContent::as_tool_response)
        .filter_map(|response| response.tool_result.as_ref().ok())
        .flat_map(|result| &result.content)
        .filter_map(|content| content.as_text())
        .filter(|text| text.text == "result: 3")
        .count();
    assert_eq!(emitted_result_count, 1);
    assert_eq!(
        api.calls().last().unwrap().input_occurrences("result: 3"),
        1
    );

    pipeline
        .seed([
            Message::user().with_text("persisted unfinished call"),
            Message::assistant().with_tool_request(
                "unfinished",
                Ok(rmcp::model::CallToolRequestParams::new(ADD)
                    .with_arguments(value(1).as_object().unwrap().clone())),
            ),
        ])
        .await?;
    let pipeline = pipeline.reconstruct().await?;
    let result = pipeline.resume_cancelled().await?;
    result.assert_message(-1, ToolResponse, "cancelled before execution");
    assert_eq!(pipeline.calculator_total(), 0);
    let request_ids = result
        .conversation()
        .messages()
        .iter()
        .flat_map(Message::get_tool_request_ids)
        .collect::<std::collections::BTreeSet<_>>();
    let response_ids = result
        .conversation()
        .messages()
        .iter()
        .flat_map(Message::get_tool_response_ids)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(response_ids, request_ids);

    Ok(())
}

#[tokio::test]
async fn elicitation_accept_decline_and_cancel() -> Result<()> {
    let (pipeline, api) = test_pipeline().await?;

    api.on("accept a value").call(REQUEST_VALUE, json!({}));
    api.on("result: 7").reply("accepted");
    let result = pipeline
        .run_with_elicitation("accept a value", ElicitationAction::Accept, value(7))
        .await?;
    result.assert_message(-2, ToolResponse, "result: 7");
    result.assert_message(-1, Agent, "accepted");
    assert_eq!(pipeline.calculator_total(), 7);

    api.on("decline a value").call(REQUEST_VALUE, json!({}));
    api.on("elicitation declined").reply("declined");
    let result = pipeline
        .run_with_elicitation("decline a value", ElicitationAction::Decline, json!({}))
        .await?;
    result.assert_message(-2, ToolResponse, "elicitation declined");
    result.assert_message(-1, Agent, "declined");

    api.on("cancel a value").call(REQUEST_VALUE, json!({}));
    api.on("elicitation cancelled").reply("cancelled");
    let result = pipeline
        .run_with_elicitation("cancel a value", ElicitationAction::Cancel, json!({}))
        .await?;
    result.assert_message(-2, ToolResponse, "elicitation cancelled");
    result.assert_message(-1, Agent, "cancelled");
    assert_eq!(pipeline.calculator_total(), 7);

    Ok(())
}

#[tokio::test]
async fn tool_availability_tracks_mode_and_extension_removal() -> Result<()> {
    let (pipeline, api) = test_pipeline().await?;

    api.on("install the extra extension").call(
        crate::agents::platform_extensions::MANAGE_EXTENSIONS_TOOL_NAME_COMPLETE,
        json!({
            "action": "enable",
            "extension_name": "analyze"
        }),
    );
    api.on("installed successfully").reply("extension ready");
    let result = pipeline.run(["install the extra extension"]).await?;
    result.assert_message(-1, Agent, "extension ready");
    assert!(api.calls().last().unwrap().advertises_tool("analyze"));

    let pipeline = pipeline.with_goose_mode(GooseMode::Chat).await;
    api.on("try the tool").call(ADD, value(1));
    api.on(CHAT_MODE_TOOL_SKIPPED_RESPONSE)
        .reply("here is the plan");

    let result = pipeline.run(["try the tool"]).await?;
    result.assert_message(-2, ToolResponse, "chat mode");
    result.assert_message(-1, Agent, "here is the plan");
    assert!(api.calls().last().unwrap().advertises_tool(ADD));
    assert_eq!(pipeline.calculator_total(), 0);

    let pipeline = pipeline.with_goose_mode(GooseMode::Auto).await;
    pipeline.remove_extension("calculator").await?;
    api.on("continue without the calculator")
        .reply("calculator removed");
    let result = pipeline.run(["continue without the calculator"]).await?;
    result.assert_message(-1, Agent, "calculator removed");
    assert!(!api.calls().last().unwrap().advertises_tool(ADD));

    Ok(())
}

#[tokio::test]
async fn stale_orphaned_tool_request_is_not_executed() -> Result<()> {
    let (pipeline, api) = test_pipeline().await?;
    api.on("are you there?").reply("fresh start");
    let session_manager = pipeline.session_manager.clone();
    session_manager
        .add_message(
            &pipeline.session_id,
            &Message::user().with_text("old prompt"),
        )
        .await?;
    session_manager
        .add_message(
            &pipeline.session_id,
            &Message::assistant().with_tool_request(
                "orphan_1",
                Ok(rmcp::model::CallToolRequestParams::new("shell")
                    .with_arguments(serde_json::Map::new())),
            ),
        )
        .await?;

    let result = pipeline.run(["are you there?"]).await?;

    assert_eq!(api.call_count(), 1);
    assert!(!api.calls()[0].input_contains("orphan_1"));

    result.assert_message(-1, Agent, "fresh start");
    let conversation = result.session.conversation.unwrap_or_default();
    assert!(conversation.messages().iter().any(|m| {
        m.content
            .iter()
            .any(|c| matches!(c, MessageContent::ToolRequest(request) if request.id == "orphan_1"))
    }));
    assert!(!conversation.messages().iter().any(|m| m.is_tool_response()));

    Ok(())
}
