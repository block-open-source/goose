use anyhow::Result;
use rmcp::model::ElicitationAction;
use serde_json::json;

use super::calculator_extension::{value, ADD, REQUEST_VALUE};
use super::dummy_api::ProviderFeatures;
use super::pipeline::test_pipeline_with;
use super::pipeline::MessageKind::{Agent, ToolCall, ToolResponse};
use crate::agents::final_output_tool::FINAL_OUTPUT_TOOL_NAME;
use crate::agents::platform_extensions::MANAGE_EXTENSIONS_TOOL_NAME_COMPLETE;
use crate::agents::tool_execution::CHAT_MODE_TOOL_SKIPPED_RESPONSE;
use crate::config::GooseMode;
use crate::conversation::message::MessageContent;
use crate::recipe::{Recipe, Response};
use goose_providers::model::ModelConfig;

#[tokio::test]
async fn tool_turn_reconstructs_after_every_applied_step() -> Result<()> {
    let (pipeline, api) = super::pipeline::test_pipeline().await?;
    api.on("add one").call(ADD, value(1));
    api.on("result: 1").reply("the total is one");

    let (pipeline, result, applied_steps) =
        pipeline.run_reconstructing_each_step("add one").await?;

    assert!(applied_steps >= 3);
    assert_eq!(api.call_count(), 2);
    result.assert_message(1, ToolCall, ADD);
    result.assert_message(2, ToolResponse, "result: 1");
    result.assert_message(-1, Agent, "the total is one");

    api.on("continue after reconstruction")
        .reply("still working");
    let continued = pipeline.run(["continue after reconstruction"]).await?;
    continued.assert_message(-1, Agent, "still working");

    Ok(())
}

#[tokio::test]
async fn reconstruction_and_session_isolation() -> Result<()> {
    let (pipeline, api) = test_pipeline_with(ProviderFeatures {
        cache_read_tokens: Some(11),
        cache_write_tokens: Some(7),
        ..ProviderFeatures::default()
    })
    .await?;
    api.on("install analyze").call(
        MANAGE_EXTENSIONS_TOOL_NAME_COMPLETE,
        json!({
            "action": "enable",
            "extension_name": "analyze"
        }),
    );
    api.on("installed successfully").reply("analyze is ready");

    let first = pipeline.run(["install analyze"]).await?;
    let first_cost = first.session.accumulated_cost.expect("estimated cost");
    assert_eq!(first.session.usage.cache_read_input_tokens, Some(11));
    assert_eq!(first.session.usage.cache_write_input_tokens, Some(7));

    let recipe = Recipe::builder()
        .title("Persisted recipe")
        .description("Persists across pipeline reconstruction")
        .instructions("Return a structured answer")
        .response(Response {
            json_schema: Some(json!({
                "type": "object",
                "properties": { "answer": { "type": "string" } },
                "required": ["answer"]
            })),
        })
        .build()
        .expect("valid recipe");
    pipeline.set_recipe(recipe).await?;
    let model_config = ModelConfig::new("gpt-4o").with_canonical_limits("openai");
    let pipeline = pipeline
        .with_model_config(model_config)
        .await
        .with_goose_mode(GooseMode::Chat)
        .await;

    let pipeline = pipeline.reconstruct().await?.with_max_turns(2);
    let restored = pipeline.session().await?;
    assert_eq!(restored.provider_name.as_deref(), Some("openai"));
    assert_eq!(
        restored
            .model_config
            .as_ref()
            .map(|config| config.model_name.as_str()),
        Some("gpt-4o")
    );
    assert_eq!(pipeline.context_limit(), 128_000);
    assert_eq!(restored.goose_mode, GooseMode::Chat);
    assert_eq!(
        restored.recipe.as_ref().map(|recipe| recipe.title.as_str()),
        Some("Persisted recipe")
    );

    api.on("check restored state").call(
        FINAL_OUTPUT_TOOL_NAME,
        json!({ "answer": "state restored" }),
    );
    api.on(CHAT_MODE_TOOL_SKIPPED_RESPONSE)
        .reply("structured output stayed disabled in chat mode");
    let restored_call_index = api.call_count();
    let restored_result = pipeline.run(["check restored state"]).await?;
    let restored_messages = restored_result.conversation().messages();
    assert!(
        restored_messages
            .iter()
            .flat_map(|message| &message.content)
            .any(|content| match content {
                MessageContent::ToolResponse(response) =>
                    response
                        .tool_result
                        .as_ref()
                        .is_ok_and(|result| result.content.iter().any(|content| {
                            content
                                .as_text()
                                .is_some_and(|text| text.text == CHAT_MODE_TOOL_SKIPPED_RESPONSE)
                        })),
                _ => false,
            }),
        "a restored Chat-mode recipe must skip its final-output tool"
    );
    assert!(
        restored_messages
            .iter()
            .all(|message| message.as_concat_text() != r#"{"answer":"state restored"}"#),
        "a skipped final-output call must not be collected after reconstruction"
    );
    let restored_call = api.calls()[restored_call_index].clone();
    assert!(restored_call.uses_model("gpt-4o"));
    assert!(restored_call.advertises_tool("analyze"));
    assert!(restored_call.advertises_tool(FINAL_OUTPUT_TOOL_NAME));

    let pipeline = pipeline.with_goose_mode(GooseMode::Auto).await;
    let pipeline = pipeline.reconstruct().await?;
    api.on("collect restored structured state").call(
        FINAL_OUTPUT_TOOL_NAME,
        json!({ "answer": "state restored" }),
    );
    let restored_auto = pipeline.run(["collect restored structured state"]).await?;
    restored_auto.assert_message(-1, Agent, r#"{"answer":"state restored"}"#);

    let pipeline = pipeline.with_goose_mode(GooseMode::Chat).await;
    pipeline
        .session_manager
        .update(&pipeline.session_id)
        .recipe(None)
        .apply()
        .await?;
    let pipeline = pipeline.reconstruct().await?;
    api.on("try the restored calculator").call(ADD, value(1));
    let next_tool_call_id = format!("dummy-tool-call-{}", api.call_count() + 1);
    api.on(next_tool_call_id)
        .reply("chat mode kept the tool idle");
    let chat = pipeline.run(["try the restored calculator"]).await?;
    chat.assert_message(-2, ToolResponse, CHAT_MODE_TOOL_SKIPPED_RESPONSE);
    chat.assert_message(-1, Agent, "chat mode kept the tool idle");
    assert_eq!(pipeline.calculator_total(), 0);

    let pipeline = pipeline.with_goose_mode(GooseMode::Auto).await;
    let pipeline = pipeline.reconstruct().await?.with_max_turns(2);
    api.on("keep adding").call(ADD, value(1));
    let calls_before = api.call_count();
    let bounded = pipeline.run(["keep adding"]).await?;
    assert_eq!(api.call_count() - calls_before, 2);
    assert_eq!(pipeline.calculator_total(), 1);
    bounded.assert_message(-1, Agent, crate::agents::state_machine::MAX_TURNS_MESSAGE);

    let restored = pipeline.session().await?;
    assert_eq!(restored.usage.cache_read_input_tokens, Some(11));
    assert_eq!(restored.usage.cache_write_input_tokens, Some(7));
    assert!(restored
        .accumulated_cost
        .is_some_and(|cost| cost > first_cost));
    assert!(pipeline.tool_contexts().iter().all(|context| {
        context.session_id == pipeline.session_id
            && context.working_dir.as_deref() == Some(pipeline.working_dir())
    }));

    let other_dir = tempfile::tempdir()?;
    let other = pipeline.new_session(other_dir.path().to_path_buf()).await?;
    api.on("add in the other session").call(ADD, value(2));
    api.on("result: 2").reply("other session added");
    other.run(["add in the other session"]).await?;
    assert_eq!(other.calculator_total(), 2);
    assert!(other.tool_contexts().iter().all(|context| {
        context.session_id == other.session_id
            && context.working_dir.as_deref() == Some(other_dir.path())
    }));
    assert_ne!(pipeline.session_id, other.session_id);
    assert_ne!(pipeline.working_dir(), other.working_dir());

    api.on("ask in the other session")
        .call(REQUEST_VALUE, json!({}));
    api.on("result: 9").reply("other session accepted");
    let elicited = other
        .run_with_elicitation(
            "ask in the other session",
            ElicitationAction::Accept,
            value(9),
        )
        .await?;
    elicited.assert_message(-2, ToolResponse, "result: 9");
    assert_eq!(other.calculator_total(), 9);
    assert_eq!(pipeline.calculator_total(), 1);

    Ok(())
}
