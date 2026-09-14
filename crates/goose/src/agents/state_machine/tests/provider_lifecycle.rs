use anyhow::Result;

use super::calculator_extension::{named_values, value, ADD, ADD_VALUES};
use super::dummy_api::ProviderFeatures;
use super::pipeline::MessageKind::{Agent, Error, Thinking, ToolCall, ToolResponse};
use super::pipeline::{test_pipeline, test_pipeline_with};
use crate::agents::AgentEvent;
use crate::conversation::fix_conversation;
use crate::conversation::message::{Message, MessageContent, MessageErrorKind};
use crate::conversation::Conversation;

#[tokio::test]
async fn provider_lifecycle() -> Result<()> {
    let (mut pipeline, api) = test_pipeline_with(ProviderFeatures {
        reports_usage: false,
        preserves_thinking: true,
        resolved_model: Some("resolved-test-model"),
        ..ProviderFeatures::default()
    })
    .await?;
    pipeline
        .set_system_prompt_override("CUSTOM_SYSTEM_PROMPT")
        .await;

    api.on("inspect this image and add one")
        .reasoning("I should inspect the image before calculating.")
        .reply("The image is suitable. I will add one.")
        .call(ADD, value(1));
    api.on("result: 1").reply("The total is 1.");

    let image_data = "aW1hZ2UtZGF0YQ==";
    let result = pipeline
        .run_message(
            Message::user()
                .with_text("inspect this image and add one")
                .with_image(image_data, "image/png"),
        )
        .await?;
    result.assert_message(2, Agent, "The image is suitable. I will add one.");
    result.assert_message(
        3,
        Thinking,
        "I should inspect the image before calculating.",
    );
    result.assert_message(4, ToolCall, ADD);
    result.assert_message(5, ToolResponse, "result: 1");
    result.assert_message(-1, Agent, "The total is 1.");
    assert!(result
        .conversation()
        .messages()
        .iter()
        .filter(|message| message.role == rmcp::model::Role::Assistant)
        .all(|message| {
            message
                .metadata
                .inference
                .as_ref()
                .and_then(|inference| inference.resolved_model.as_deref())
                == Some("resolved-test-model")
        }));

    let calls = api.calls();
    assert!(calls[0].input_has_image("image/png", image_data));
    assert!(calls[..2]
        .iter()
        .all(|call| call.system_contains("CUSTOM_SYSTEM_PROMPT")));
    assert_eq!(
        calls[1].input_occurrences("I should inspect the image before calculating."),
        1
    );
    assert_eq!(
        calls[1].input_occurrences("The image is suitable. I will add one."),
        1
    );
    assert_eq!(calls[1].input_occurrences(ADD), 1);
    let schema = calls[0].tool_schema(ADD_VALUES).expect("add_values schema");
    assert!(schema.get("additionalProperties").is_some());
    assert!(schema
        .get("properties")
        .and_then(serde_json::Value::as_object)
        .is_none_or(serde_json::Map::is_empty));

    api.on("make a malformed mixed call")
        .reasoning("I should preserve this reasoning.")
        .reply("I will try the tool.")
        .malformed_call(ADD, r#"{"value":"#);
    api.on("could not be parsed")
        .reply("I recovered from the malformed call.");
    let result = pipeline.run(["make a malformed mixed call"]).await?;
    result.assert_message(-2, ToolResponse, "could not be parsed");
    result.assert_message(-1, Agent, "I recovered from the malformed call.");

    let mixed_response = result
        .conversation()
        .messages()
        .iter()
        .find(|message| {
            message.content.iter().any(
                |content| matches!(content, MessageContent::ToolRequest(request) if request.tool_call.is_err()),
            )
        })
        .expect("mixed response with malformed tool call");
    assert!(
        mixed_response
            .id
            .as_deref()
            .is_some_and(|id| id.starts_with("chatcmpl-test-")),
        "provider response id was not preserved: {:?}",
        mixed_response.id
    );
    let emitted_mixed_response = result
        .events
        .iter()
        .find_map(|event| match event {
            AgentEvent::Message(message)
                if message.content.iter().any(
                    |content| matches!(content, MessageContent::ToolRequest(request) if request.tool_call.is_err()),
                ) =>
            {
                Some(message)
            }
            _ => None,
        })
        .expect("emitted mixed response with malformed tool call");
    assert_eq!(emitted_mixed_response.id, mixed_response.id);
    assert!(result.events.iter().any(|event| {
        matches!(
            event,
            AgentEvent::MessageUsage { message_id, .. }
                if message_id == &mixed_response.id
        )
    }));
    assert_eq!(
        mixed_response
            .content
            .iter()
            .filter_map(|content| match content {
                MessageContent::Thinking(thinking) => Some(thinking.thinking.as_str()),
                _ => None,
            })
            .collect::<String>(),
        "I should preserve this reasoning."
    );
    assert_eq!(
        mixed_response
            .content
            .iter()
            .filter_map(|content| match content {
                MessageContent::Text(text) => Some(text.text.as_str()),
                _ => None,
            })
            .collect::<String>(),
        "I will try the tool."
    );
    let calls = api.calls();
    let recovery_call = calls.last().expect("malformed-call recovery request");
    assert_eq!(
        recovery_call.input_occurrences("I should preserve this reasoning."),
        1
    );
    assert_eq!(recovery_call.input_occurrences("I will try the tool."), 1);
    let messages = result.conversation().messages();
    let provider_input = Conversation::new_unvalidated(messages[..messages.len() - 1].to_vec());
    let (_, repairs) = fix_conversation(provider_input);
    assert!(
        repairs.is_empty(),
        "state-machine conversation needed repairs: {repairs:?}"
    );

    api.on("add named values")
        .call(ADD_VALUES, named_values([("left", 2), ("right", 3)]));
    api.on("result: 6").reply("The total is 6.");
    let result = pipeline.run(["add named values"]).await?;
    result.assert_message(-2, ToolResponse, "result: 6");
    result.assert_message(-1, Agent, "The total is 6.");

    assert!(result.session.usage.total_tokens.is_none());
    assert!(result
        .session
        .conversation
        .as_ref()
        .and_then(Conversation::last)
        .and_then(|message| message.metadata.usage.as_ref())
        .and_then(|usage| usage.total_tokens)
        .is_none());

    api.on("return no choices").no_choices();
    let result = pipeline.run(["return no choices"]).await?;
    result.assert_message(-1, Agent, "model returned an empty response");

    api.on("after no choices")
        .reply("recovered from no choices");
    let result = pipeline.run(["after no choices"]).await?;
    result.assert_message(-1, Agent, "recovered from no choices");

    api.on("return an empty reply").reply("");
    let result = pipeline.run(["return an empty reply"]).await?;
    result.assert_message(-1, Agent, "model returned an empty response");

    api.on("after empty reply")
        .reply("recovered from empty reply");
    let result = pipeline.run(["after empty reply"]).await?;
    result.assert_message(-1, Agent, "recovered from empty reply");

    api.on("hit the output limit").output_limit();
    let result = pipeline.run(["hit the output limit"]).await?;
    let marker = result.conversation().last().expect("output-limit marker");
    assert!(marker.metadata.output_token_limit_reached);
    assert!(marker.content.is_empty());
    assert!(result.events.iter().any(|event| {
        matches!(event, AgentEvent::Message(message) if message.metadata.output_token_limit_reached)
    }));

    api.on("return an empty server error").empty_server_error();
    let result = pipeline.run(["return an empty server error"]).await?;
    result.assert_message(-1, Error, "500");

    api.on("after server error")
        .reply("recovered from server error");
    let result = pipeline.run(["after server error"]).await?;
    result.assert_message(-1, Agent, "recovered from server error");
    assert!(result.session.usage.total_tokens.is_none());
    assert!(api
        .calls()
        .iter()
        .all(|call| call.system_contains("CUSTOM_SYSTEM_PROMPT")));

    pipeline.clear_system_prompt_override().await;
    pipeline = pipeline.with_model("gpt-4.1").await;
    api.on("use the standard prompt")
        .reply("The standard prompt is active.");
    let result = pipeline.run(["use the standard prompt"]).await?;
    result.assert_message(-1, Agent, "The standard prompt is active.");
    let call = api.calls().last().cloned().expect("provider request");
    assert!(call.uses_model("gpt-4.1"));
    assert!(call.system_contains("general-purpose AI agent called goose"));

    Ok(())
}

#[tokio::test]
async fn usage_and_provider_errors_survive_persistence() -> Result<()> {
    let (pipeline, api) = test_pipeline().await?;
    api.on("hello").reply("hi there");

    let result = pipeline.run(["hello"]).await?;
    let input_tokens = api.calls()[0].input_tokens();
    let output_tokens = "hi there".chars().count() as i32;
    let total_tokens = input_tokens + output_tokens;

    assert_eq!(result.session.usage.total_tokens, Some(total_tokens));
    assert_eq!(result.session.usage.input_tokens, Some(input_tokens));
    assert_eq!(result.session.usage.output_tokens, Some(output_tokens));
    assert!(result.events.iter().any(
        |event| matches!(event, AgentEvent::Usage(usage) if usage.usage.total_tokens == Some(total_tokens))
    ));
    assert!(result
        .events
        .iter()
        .any(|event| matches!(event, AgentEvent::MessageUsage { .. })));
    let assistant = result
        .conversation()
        .messages()
        .iter()
        .find(|message| message.role == rmcp::model::Role::Assistant)
        .expect("assistant response");
    assert_eq!(
        assistant
            .metadata
            .usage
            .as_ref()
            .and_then(|usage| usage.total_tokens),
        Some(total_tokens)
    );

    api.on("stream then fail")
        .reply("partial response")
        .server_error("boom");
    let result = pipeline.run(["stream then fail"]).await?;
    let stream_total =
        api.calls().last().unwrap().input_tokens() + "partial response".chars().count() as i32;
    assert_eq!(result.session.usage.total_tokens, Some(stream_total));
    assert!(result.events.iter().any(
        |event| matches!(event, AgentEvent::Usage(usage) if usage.usage.total_tokens == Some(stream_total))
    ));
    assert!(result
        .events
        .iter()
        .any(|event| matches!(event, AgentEvent::MessageUsage { .. })));
    result.assert_message(-2, Agent, "partial response");
    let error = result
        .conversation()
        .messages()
        .iter()
        .find(|message| message.error_kind().is_some())
        .expect("persisted stream error");
    assert!(error.metadata.usage.is_none());

    api.on("fail immediately").server_error("immediate boom");
    let result = pipeline.run(["fail immediately"]).await?;
    result.assert_message(-1, Error, "immediate boom");
    let error = result
        .conversation()
        .messages()
        .iter()
        .find(|message| message.error_kind() == Some(MessageErrorKind::Other))
        .expect("persisted provider error");
    assert!(error.is_user_visible());
    assert!(!error.is_agent_visible());

    Ok(())
}

#[tokio::test]
async fn requested_model_is_recorded_without_resolved_model() -> Result<()> {
    let (pipeline, api) = test_pipeline().await?;
    api.on("hello").reply("hi there");

    let result = pipeline.run(["hello"]).await?;
    let requested_model = &result.session.model_config.as_ref().unwrap().model_name;
    let inference = result
        .conversation()
        .messages()
        .iter()
        .find(|message| message.role == rmcp::model::Role::Assistant)
        .and_then(|message| message.metadata.inference.as_ref())
        .expect("assistant inference metadata");

    assert_eq!(&inference.requested_model, requested_model);
    assert_eq!(inference.resolved_model, None);
    Ok(())
}
