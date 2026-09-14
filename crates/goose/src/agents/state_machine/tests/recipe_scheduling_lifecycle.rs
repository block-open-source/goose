use anyhow::Result;
use serde_json::json;

use super::dummy_api::{DummyApi, ProviderFeatures};
use super::pipeline::{
    MessageKind::Agent, MessageKind::Error, MessageKind::ToolResponse, test_pipeline,
    test_pipeline_with, test_pipeline_with_scheduler,
};
use crate::agents::extension::ExtensionConfig;
use crate::agents::final_output_tool::{FINAL_OUTPUT_CONTINUATION_MESSAGE, FINAL_OUTPUT_TOOL_NAME};
use crate::agents::platform_extensions::scheduler::MANAGE_SCHEDULE_TOOL_NAME_COMPLETE;
#[cfg(feature = "code-mode")]
use crate::agents::state_machine::ops_tool_approval::TOOL_EXECUTABLE_KEY;
use crate::agents::state_machine::MAX_TURNS_MESSAGE;
use crate::agents::tool_execution::CHAT_MODE_TOOL_SKIPPED_RESPONSE;
use crate::agents::types::{RetryConfig, SuccessCheck};
#[cfg(feature = "code-mode")]
use crate::config::permission::PermissionLevel;
use crate::config::GooseMode;
#[cfg(feature = "code-mode")]
use crate::conversation::message::MessageContent;
use crate::recipe::build_recipe::build_recipe_from_template;
use crate::recipe::{Recipe, Response, SubRecipe};

#[tokio::test]
async fn inherited_recipe_parameters_instructions_and_extensions_reach_inference() -> Result<()> {
    let (pipeline, api) = test_pipeline().await?;
    let parent = pipeline.working_dir().join("parent.yaml");
    let child = pipeline.working_dir().join("child.yaml");
    std::fs::write(
        &parent,
        r#"
version: 1.0.0
title: Parent
description: Parent recipe
instructions: Follow the inherited workflow exactly.
prompt: |
  Plan work for {{ date }} with {{ detail }} detail.
  {% block task %}Use the parent task.{% endblock %}
parameters:
  - key: date
    input_type: string
    requirement: required
    description: Date to plan
  - key: detail
    input_type: string
    requirement: optional
    description: Desired detail
    default: brief
extensions:
  - type: platform
    name: analyze
    description: Analyze code
    display_name: Analyze
    bundled: true
    available_tools: []
"#,
    )?;
    std::fs::write(
        &child,
        r#"
{% extends "parent.yaml" %}
{% block task %}Use the child task.{% endblock %}
"#,
    )?;

    let recipe = build_recipe_from_template(
        std::fs::read_to_string(child)?,
        pipeline.working_dir(),
        vec![("date".to_string(), "Friday".to_string())],
        None::<fn(&str, &str) -> Result<String>>,
    )?;
    let prompt = [recipe.instructions.as_deref(), recipe.prompt.as_deref()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("\n\n");
    pipeline.set_recipe(recipe).await?;
    api.on("Use the child task.").reply("planned");

    let result = pipeline.run([prompt.as_str()]).await?;
    result.assert_message(-1, Agent, "planned");
    let call = &api.calls()[0];
    assert!(call.input_contains("Follow the inherited workflow exactly."));
    assert!(call.input_contains("Plan work for Friday with brief detail."));
    assert!(call.input_contains("Use the child task."));
    assert!(call.advertises_tool("analyze"));

    Ok(())
}

#[tokio::test]
async fn recipe_delegation_respects_mode_and_child_turn_limit() -> Result<()> {
    let (pipeline, api) = test_pipeline().await?;
    let pipeline = pipeline.with_goose_mode(GooseMode::Chat).await;
    let recipe = Recipe::builder()
        .title("Chat recipe")
        .description("Chat recipe")
        .prompt("Try to delegate")
        .extensions(vec![ExtensionConfig::Platform {
            name: "summon".to_string(),
            description: "Delegate work".to_string(),
            display_name: None,
            bundled: None,
            available_tools: vec![],
        }])
        .build()
        .expect("valid recipe");
    pipeline.set_recipe(recipe).await?;
    api.on("Try to delegate")
        .call("delegate", json!({ "instructions": "Do the work" }));
    api.on(CHAT_MODE_TOOL_SKIPPED_RESPONSE)
        .reply("delegation stayed in chat");

    let result = pipeline.run(["Try to delegate"]).await?;
    result.assert_message(-2, ToolResponse, CHAT_MODE_TOOL_SKIPPED_RESPONSE);
    result.assert_message(-1, Agent, "delegation stayed in chat");

    let (pipeline, api) = test_pipeline().await?;
    let pipeline = pipeline.with_provider_name("state-machine-test").await?;
    let child_path = pipeline.working_dir().join("bounded-child.yaml");
    std::fs::write(
        &child_path,
        r#"
version: 1.0.0
title: Bounded child
description: Stops after one turn
prompt: Keep taking actions
settings:
  max_turns: 1
"#,
    )?;
    let recipe = Recipe::builder()
        .title("Delegating recipe")
        .description("Delegates bounded work")
        .prompt("Delegate the bounded child")
        .extensions(vec![ExtensionConfig::Platform {
            name: "summon".to_string(),
            description: "Delegate work".to_string(),
            display_name: None,
            bundled: None,
            available_tools: vec![],
        }])
        .sub_recipes(vec![SubRecipe {
            name: "bounded".to_string(),
            path: child_path.to_string_lossy().into_owned(),
            values: None,
            sequential_when_repeated: false,
            description: None,
        }])
        .build()
        .expect("valid recipe");
    pipeline.set_recipe(recipe).await?;
    api.on("Delegate the bounded child")
        .call("delegate", json!({ "source": "bounded" }));
    api.on("Keep taking actions")
        .unadvertised_call("keep_working", json!({}));
    api.on(MAX_TURNS_MESSAGE).reply("child stopped on time");

    let result = pipeline.run(["Delegate the bounded child"]).await?;
    assert_eq!(api.call_count(), 3);
    result.assert_message(-2, ToolResponse, MAX_TURNS_MESSAGE);
    result.assert_message(-1, Agent, "child stopped on time");

    Ok(())
}

#[tokio::test]
async fn recipe_retry_and_final_output_run_to_completion() -> Result<()> {
    let (pipeline, api) = test_pipeline().await?;
    api.on("do the thing").reply("attempt");
    let recipe = Recipe::builder()
        .title("retry test")
        .description("retry test")
        .prompt("do the thing")
        .retry(RetryConfig {
            max_retries: 1,
            checks: vec![SuccessCheck::Shell {
                command: "exit 1".to_string(),
            }],
            on_failure: None,
            timeout_seconds: None,
            on_failure_timeout_seconds: None,
        })
        .build()
        .expect("valid recipe");
    pipeline.set_recipe(recipe).await?;

    let (_pipeline, exhausted, _) = pipeline
        .run_reconstructing_each_step("do the thing")
        .await?;
    assert_eq!(exhausted.history_replacements(), 1);
    assert_eq!(api.call_count(), 2);
    exhausted.assert_message(-1, Error, "Maximum retry attempts (1) exceeded");

    let (pipeline, api) = test_pipeline().await?;
    api.on("compute the answer").reply("thinking about it");
    api.on(FINAL_OUTPUT_CONTINUATION_MESSAGE)
        .call(FINAL_OUTPUT_TOOL_NAME, json!({ "result": "42" }));
    let recipe = Recipe::builder()
        .title("Structured output")
        .description("Return structured output")
        .instructions("Compute the answer")
        .response(Response {
            json_schema: Some(json!({
                "type": "object",
                "properties": { "result": { "type": "string" } },
                "required": ["result"]
            })),
        })
        .build()
        .expect("valid recipe");
    pipeline.set_recipe(recipe).await?;

    let completed = pipeline.run(["compute the answer"]).await?;
    assert_eq!(api.call_count(), 2);
    assert!(api.calls()[0].advertises_tool(FINAL_OUTPUT_TOOL_NAME));
    assert!(api.calls()[1].advertises_tool(FINAL_OUTPUT_TOOL_NAME));
    completed.assert_message(-1, Agent, r#"{"result":"42"}"#);

    Ok(())
}

#[cfg(feature = "code-mode")]
#[tokio::test]
async fn unadvertised_final_output_is_neither_approved_nor_executed() -> Result<()> {
    let (pipeline, api) = test_pipeline().await?;
    let recipe = Recipe::builder()
        .title("Structured output boundary")
        .description("Structured output boundary")
        .instructions("Return structured output")
        .response(Response {
            json_schema: Some(json!({
                "type": "object",
                "properties": { "result": { "type": "string" } },
                "required": ["result"]
            })),
        })
        .build()
        .expect("valid recipe");
    pipeline.set_recipe(recipe).await?;
    pipeline.add_extension("code_execution").await?;
    pipeline.set_permission(FINAL_OUTPUT_TOOL_NAME, PermissionLevel::AlwaysAllow);
    let pipeline = pipeline
        .with_max_turns(2)
        .with_goose_mode(GooseMode::Approve)
        .await;
    api.on("try the hidden final output")
        .unadvertised_call(FINAL_OUTPUT_TOOL_NAME, json!({ "result": "escaped" }));
    api.on("not available").reply("recipe call blocked");

    let result = pipeline.run(["try the hidden final output"]).await?;

    assert!(!api.calls()[0].advertises_tool(FINAL_OUTPUT_TOOL_NAME));
    assert!(result.conversation().messages().iter().any(|message| {
        message.content.iter().any(|content| {
            matches!(content, MessageContent::ToolResponse(response)
            if response.tool_result.as_ref().is_ok_and(|result| {
                result.content.iter().any(|content| {
                    content.as_text().is_some_and(|text| {
                        text.text.contains("Tool 'recipe__final_output' is not available")
                    })
                })
            }))
        })
    }));
    assert!(result.conversation().messages().iter().any(|message| {
        message.content.iter().any(|content| {
            matches!(content, MessageContent::Text(text) if text.text == "recipe call blocked")
        })
    }));
    assert!(!result.conversation().messages().iter().any(|message| {
        message
            .content
            .iter()
            .any(|content| matches!(content, MessageContent::ActionRequired(_)))
    }));
    let request = result
        .conversation()
        .messages()
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(MessageContent::as_tool_request)
        .find(|request| {
            request
                .tool_call
                .as_ref()
                .is_ok_and(|tool_call| tool_call.name == FINAL_OUTPUT_TOOL_NAME)
        })
        .expect("final_output request");
    assert!(request
        .tool_meta
        .as_ref()
        .and_then(|metadata| metadata.get(TOOL_EXECUTABLE_KEY))
        .is_none());

    Ok(())
}

#[tokio::test]
async fn structured_output_fails_fast_when_provider_manages_own_context() -> Result<()> {
    let (pipeline, api) = test_pipeline_with(ProviderFeatures {
        manages_own_context: true,
        ..ProviderFeatures::default()
    })
    .await?;
    let pipeline = pipeline.with_provider_name("context-owning-test").await?;
    api.on("compute the answer").reply("thinking about it");
    api.on(FINAL_OUTPUT_CONTINUATION_MESSAGE)
        .call(FINAL_OUTPUT_TOOL_NAME, json!({ "result": "42" }));
    let recipe = Recipe::builder()
        .title("Structured output")
        .description("Return structured output")
        .instructions("Compute the answer")
        .response(Response {
            json_schema: Some(json!({
                "type": "object",
                "properties": { "result": { "type": "string" } },
                "required": ["result"]
            })),
        })
        .build()
        .expect("valid recipe");
    pipeline.set_recipe(recipe).await?;

    let result = pipeline.run(["compute the answer"]).await?;
    result.assert_message(-1, Agent, "provider `context-owning-test` can't support it");
    assert!(
        api.calls()
            .iter()
            .all(|call| !call.input_contains(FINAL_OUTPUT_CONTINUATION_MESSAGE)),
        "must fail fast without ever entering the continuation-nudge loop"
    );

    Ok(())
}

#[tokio::test]
async fn scheduler_is_advertised_only_when_configured_and_manages_jobs() -> Result<()> {
    let (pipeline, api) = test_pipeline().await?;
    api.on("can I schedule work?").reply("not here");

    pipeline.run(["can I schedule work?"]).await?;
    assert!(!api.calls()[0].advertises_tool(MANAGE_SCHEDULE_TOOL_NAME_COMPLETE));

    let (pipeline, api, scheduler) = test_pipeline_with_scheduler().await?;
    let recipe_path = pipeline.working_dir().join("scheduled.yaml");
    std::fs::write(
        &recipe_path,
        "version: 1.0.0\n\
         title: Scheduled task\n\
         description: A scheduled task\n\
         prompt: Do the scheduled work\n",
    )?;
    let recipe_path = recipe_path.to_string_lossy();

    api.on("create a schedule").call(
        MANAGE_SCHEDULE_TOOL_NAME_COMPLETE,
        json!({
            "action": "create",
            "recipe_path": recipe_path,
            "cron_expression": "0 9 * * *"
        }),
    );
    api.on("Successfully created scheduled job")
        .reply("schedule created");

    let created = pipeline.run(["create a schedule"]).await?;
    created.assert_message(-2, ToolResponse, "Successfully created scheduled job");
    created.assert_message(-1, Agent, "schedule created");
    assert!(api.calls()[0].advertises_tool(MANAGE_SCHEDULE_TOOL_NAME_COMPLETE));
    let jobs = scheduler.list_scheduled_jobs().await;
    assert_eq!(jobs.len(), 1);
    let job_id = jobs[0].id.clone();
    pipeline.set_schedule_id(job_id.clone()).await?;

    api.on("list schedules").call(
        MANAGE_SCHEDULE_TOOL_NAME_COMPLETE,
        json!({ "action": "list" }),
    );
    api.on("Scheduled Jobs").reply("one schedule");

    let listed = pipeline.run(["list schedules"]).await?;
    listed.assert_message(-2, ToolResponse, r#""cron": "0 9 * * *""#);
    listed.assert_message(-1, Agent, "one schedule");
    let scheduled_sessions = scheduler.sessions(&job_id, 1).await?;
    assert_eq!(scheduled_sessions.len(), 1);
    assert_eq!(
        scheduled_sessions[0].1.message_count,
        listed
            .conversation()
            .messages()
            .iter()
            .filter(|message| message.is_user_visible())
            .count()
    );
    api.on("list scheduled runs").call(
        MANAGE_SCHEDULE_TOOL_NAME_COMPLETE,
        json!({ "action": "sessions", "job_id": job_id }),
    );
    api.on("Sessions for job")
        .reply("the scheduled run was persisted");

    let sessions = pipeline.run(["list scheduled runs"]).await?;
    sessions.assert_message(-2, ToolResponse, "Messages:");
    sessions.assert_message(-1, Agent, "the scheduled run was persisted");

    api.on("create a broken schedule").call(
        MANAGE_SCHEDULE_TOOL_NAME_COMPLETE,
        json!({
            "action": "create",
            "recipe_path": recipe_path,
            "cron_expression": "not a cron expression"
        }),
    );
    api.on("Failed to create job").reply("schedule rejected");

    let rejected = pipeline.run(["create a broken schedule"]).await?;
    rejected.assert_message(-2, ToolResponse, "Invalid cron");
    rejected.assert_message(-1, Agent, "schedule rejected");

    let empty_recipe_path = pipeline.working_dir().join("empty.yaml");
    std::fs::write(
        &empty_recipe_path,
        "version: 1.0.0\n\
         title: Empty scheduled task\n\
         description: Nothing to run\n",
    )?;
    api.on("schedule an empty recipe").call(
        MANAGE_SCHEDULE_TOOL_NAME_COMPLETE,
        json!({
            "action": "create",
            "recipe_path": empty_recipe_path,
            "cron_expression": "0 9 * * *"
        }),
    );
    api.on("Recipe must specify").reply("empty recipe rejected");

    let rejected = pipeline.run(["schedule an empty recipe"]).await?;
    rejected.assert_message(-2, ToolResponse, "Recipe must specify");
    rejected.assert_message(-1, Agent, "empty recipe rejected");

    Ok(())
}

#[tokio::test]
async fn invalid_final_output_schema_stops_before_inference() -> Result<()> {
    let (pipeline, api) = test_pipeline().await?;
    let recipe = Recipe::builder()
        .title("Invalid output")
        .description("Invalid output")
        .instructions("This must not reach inference")
        .response(Response {
            json_schema: Some(json!({})),
        })
        .build()
        .expect("recipe shape is otherwise valid");
    pipeline.set_recipe(recipe).await?;

    let error = match pipeline.run(["start"]).await {
        Ok(_) => panic!("invalid schema must be rejected"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("empty json_schema"));
    assert_eq!(api.call_count(), 0);

    Ok(())
}

#[tokio::test]
async fn boolean_final_output_schema_stops_before_inference() -> Result<()> {
    let (pipeline, api) = test_pipeline().await?;
    let recipe = Recipe::builder()
        .title("Boolean output schema")
        .description("Boolean output schema")
        .instructions("This must not reach inference")
        .response(Response {
            json_schema: Some(json!(true)),
        })
        .build()
        .expect("recipe shape is otherwise valid");
    pipeline.set_recipe(recipe).await?;

    let error = match pipeline.run(["start"]).await {
        Ok(_) => panic!("boolean schema must be rejected"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("json_schema must be an object"));
    assert_eq!(api.call_count(), 0);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scheduled_run_attaches_recipe_to_session_before_inference() -> Result<()> {
    let api = DummyApi::start(ProviderFeatures::default()).await;
    let gate = api
        .on("Reply while the response gate is held.")
        .hold_reply("done");
    let host = api.uri();
    let _guard = env_lock::lock_env([
        ("GOOSE_PROVIDER", Some("openai")),
        ("GOOSE_MODEL", Some("gpt-4o")),
        ("OPENAI_API_KEY", Some("fake-openai-no-keyring")),
        ("OPENAI_HOST", Some(host.as_str())),
        ("OPENAI_CUSTOM_HEADERS", Some("")),
    ]);

    let temp_dir = tempfile::tempdir()?;
    let recipe_path = temp_dir.path().join("scheduled.yaml");
    std::fs::write(
        &recipe_path,
        r#"version: 1.0.0
title: Ordering guard
description: Scheduled recipe with a sub-recipe
prompt: Reply while the response gate is held.
extensions: []
sub_recipes:
  - name: check_calendar
    path: ./check_calendar.yaml
"#,
    )?;
    std::fs::write(
        temp_dir.path().join("check_calendar.yaml"),
        r#"version: 1.0.0
title: Check calendar
description: Sub-recipe
prompt: check
"#,
    )?;

    let session_manager = std::sync::Arc::new(crate::session::SessionManager::new(
        temp_dir.path().to_path_buf(),
    ));
    let scheduler = crate::scheduler::Scheduler::new(
        temp_dir.path().join("schedule.json"),
        session_manager.clone(),
    )
    .await?;
    let job = crate::scheduler::ScheduledJob {
        id: "ordering_guard".to_string(),
        source: recipe_path.to_string_lossy().into_owned(),
        cron: "0 0 0 1 1 *".to_string(),
        last_run: None,
        currently_running: false,
        paused: false,
        current_session_id: None,
        process_start_time: None,
        parameters: vec![],
        recipe_base_dir: None,
    };
    scheduler.add_scheduled_job(job, true).await?;

    let run = tokio::spawn({
        let scheduler = scheduler.clone();
        async move { scheduler.run_now("ordering_guard").await }
    });

    gate.entered().await;
    let sampled = async {
        let sessions = session_manager
            .list_sessions_by_types(&[crate::session::SessionType::Scheduled])
            .await?;
        session_manager.get_session(&sessions[0].id, false).await
    }
    .await;
    gate.release();
    let session = sampled?;

    let recipe = session
        .recipe
        .expect("recipe must be attached to the session before inference begins");
    assert_eq!(recipe.title, "Ordering guard");
    let sub_recipes = recipe
        .sub_recipes
        .expect("attached recipe must keep its sub_recipes block");
    assert_eq!(sub_recipes.len(), 1);
    assert_eq!(sub_recipes[0].name, "check_calendar");

    run.await??;

    Ok(())
}
