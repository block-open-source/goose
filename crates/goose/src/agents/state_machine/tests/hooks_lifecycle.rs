use anyhow::Result;
use rmcp::model::{Annotations, Role, TextContent};
use serde_json::Value;

use super::calculator_extension::{value, ADD};
use super::pipeline::{test_pipeline, MessageKind::Agent, MessageKind::ToolResponse, MAX_TURNS};
use crate::agents::final_output_tool::FINAL_OUTPUT_TOOL_NAME;
use crate::agents::platform_extensions::MANAGE_EXTENSIONS_TOOL_NAME_COMPLETE;
use crate::agents::state_machine::ops_stop_hook::DENIED;
use crate::agents::state_machine::ops_unknown_tool::UNCLAIMED_TOOL_ERROR;
use crate::agents::tool_execution::{CHAT_MODE_TOOL_SKIPPED_RESPONSE, DECLINED_RESPONSE};
use crate::config::permission::PermissionLevel;
use crate::config::GooseMode;
use crate::conversation::message::{Message, MessageContent, SystemNotificationType};
use crate::permission::Permission;

pub(super) struct HookTestEnv {
    _temp_dir: tempfile::TempDir,
    plugin_dir: std::path::PathBuf,
}

impl HookTestEnv {
    pub(super) fn new(event: &str, script: &str) -> Self {
        let temp_dir = tempfile::tempdir().unwrap();
        let plugin_dir = temp_dir.path().join("test-plugin");
        std::fs::create_dir_all(plugin_dir.join("hooks")).unwrap();
        std::fs::write(
            plugin_dir.join("hooks/hooks.json"),
            format!(
                r#"{{"hooks": {{"{event}": [{{"hooks": [{{"type": "command", "command": "sh ${{PLUGIN_ROOT}}/hook.sh"}}]}}]}}}}"#
            ),
        )
        .unwrap();
        std::fs::write(plugin_dir.join("hook.sh"), script).unwrap();
        Self {
            _temp_dir: temp_dir,
            plugin_dir,
        }
    }

    pub(super) fn hook_manager(&self) -> crate::hooks::HookManager {
        use crate::plugins::discovery::{DiscoveredPlugin, PluginScope};
        crate::hooks::HookManager::from_plugins_for_test(vec![DiscoveredPlugin {
            name: "test-plugin".into(),
            root: self.plugin_dir.clone(),
            scope: PluginScope::Project,
        }])
    }

    fn invocations(&self) -> usize {
        std::fs::read_to_string(self.plugin_dir.join("hook.log"))
            .unwrap_or_default()
            .lines()
            .count()
    }
    fn last_context(&self) -> serde_json::Value {
        serde_json::from_str(
            &std::fs::read_to_string(self.plugin_dir.join("context.json"))
                .expect("hook context was recorded"),
        )
        .expect("hook context is valid JSON")
    }

    fn payloads(&self) -> Vec<Value> {
        std::fs::read_to_string(self.plugin_dir.join("hook.log"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).expect("hook payload should be JSON"))
            .collect()
    }
}

const LOG_AND_ALLOW_SCRIPT: &str = "#!/bin/sh\necho ran >> \"$PLUGIN_ROOT/hook.log\"\nexit 0\n";
const LOG_AND_BLOCK_SCRIPT: &str =
    "#!/bin/sh\necho blocked >> \"$PLUGIN_ROOT/hook.log\"\necho \"not done yet\" >&2\nexit 2\n";
const LOG_CONTEXT_AND_BLOCK_SCRIPT: &str = "#!/bin/sh\ncat > \"$PLUGIN_ROOT/context.json\"\necho blocked >> \"$PLUGIN_ROOT/hook.log\"\necho \"not done yet\" >&2\nexit 2\n";
const RECORD_AND_ALLOW_SCRIPT: &str =
    "#!/bin/sh\npayload=$(cat)\nprintf '%s\\n' \"$payload\" >> \"$PLUGIN_ROOT/hook.log\"\nexit 0\n";
const RECORD_AND_BLOCK_MARKER_SCRIPT: &str = "#!/bin/sh
payload=$(cat)
printf '%s\\n' \"$payload\" >> \"$PLUGIN_ROOT/hook.log\"
case \"$payload\" in
  *\"policy marker\"*) echo \"policy marker found\" >&2; exit 2 ;;
esac
exit 0
";

#[tokio::test]
async fn stop_hooks_allow_block_and_skip_non_stop_exits() -> Result<()> {
    let allowed = HookTestEnv::new("Stop", LOG_AND_ALLOW_SCRIPT);
    let (pipeline, api) = test_pipeline().await?;
    let pipeline = pipeline.with_hook_manager(allowed.hook_manager());
    api.on("hello").reply("done");

    let result = pipeline.run(["hello"]).await?;
    result.assert_message(-1, Agent, "done");
    assert_eq!(api.call_count(), 1);
    assert_eq!(allowed.invocations(), 1);

    let blocked = HookTestEnv::new("Stop", LOG_CONTEXT_AND_BLOCK_SCRIPT);
    let (pipeline, api) = test_pipeline().await?;
    let expected_working_dir = pipeline.working_dir().to_string_lossy().into_owned();
    let pipeline = pipeline
        .with_hook_manager(blocked.hook_manager())
        .with_stop_hook_block_cap(2);
    api.on("hello").reply("response");
    api.on("blocked ending this turn").reply("response");

    let (_, result, _) = pipeline.run_reconstructing_each_step("hello").await?;
    assert_eq!(api.call_count(), 3);
    assert_eq!(blocked.invocations(), 3);
    assert_eq!(blocked.last_context()["working_dir"], expected_working_dir);
    assert_eq!(
        result
            .conversation()
            .messages()
            .iter()
            .filter(|message| message
                .as_concat_text()
                .contains("blocked ending this turn"))
            .count(),
        2
    );
    assert_eq!(
        result
            .conversation()
            .messages()
            .iter()
            .filter(|message| message
                .metadata
                .operation_note("stop_hook", DENIED)
                .is_some())
            .count(),
        2
    );
    assert!(result.conversation().last().is_some_and(|message| {
        message.content.iter().any(|content| {
            matches!(
                content,
                MessageContent::SystemNotification(notification)
                    if notification.notification_type == SystemNotificationType::InlineMessage
                        && notification.msg.contains("GOOSE_STOP_HOOK_BLOCK_CAP")
            )
        })
    }));

    let maxed = HookTestEnv::new("Stop", LOG_AND_ALLOW_SCRIPT);
    let (pipeline, api) = test_pipeline().await?;
    let pipeline = pipeline.with_hook_manager(maxed.hook_manager());
    api.on("keep going").call(ADD, value(1));

    pipeline.run(["keep going"]).await?;
    assert_eq!(api.call_count(), MAX_TURNS as usize);
    assert_eq!(pipeline.calculator_total(), MAX_TURNS as i64 - 1);
    assert_eq!(maxed.invocations(), 0);

    Ok(())
}

#[tokio::test]
async fn stop_hook_receives_complete_user_visible_assistant_response() -> Result<()> {
    let same_id = HookTestEnv::new("Stop", RECORD_AND_ALLOW_SCRIPT);
    let (pipeline, api) = test_pipeline().await?;
    let pipeline = pipeline.with_hook_manager(same_id.hook_manager());
    api.on("same id").reply("one two three four five");

    let result = pipeline.run(["same id"]).await?;
    let response_ids = result
        .conversation()
        .messages()
        .iter()
        .filter(|message| message.role == Role::Assistant && !message.as_concat_text().is_empty())
        .filter_map(|message| message.id.as_deref())
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(response_ids.len(), 1);
    assert_eq!(
        same_id.payloads()[0]["last_assistant_message"],
        "one two three four five"
    );

    let distinct_ids = HookTestEnv::new("Stop", RECORD_AND_ALLOW_SCRIPT);
    let (pipeline, api) = test_pipeline().await?;
    let pipeline = pipeline.with_hook_manager(distinct_ids.hook_manager());
    api.on("distinct ids")
        .reply_with_distinct_ids(["one ", "two ", "three"]);

    let result = pipeline.run(["distinct ids"]).await?;
    let response_ids = result
        .conversation()
        .messages()
        .iter()
        .filter(|message| message.role == Role::Assistant && !message.as_concat_text().is_empty())
        .filter_map(|message| message.id.as_deref())
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(response_ids.len(), 3);
    assert_eq!(
        distinct_ids.payloads()[0]["last_assistant_message"],
        "one two three"
    );

    let visibility = HookTestEnv::new("Stop", RECORD_AND_ALLOW_SCRIPT);
    let (pipeline, _) = test_pipeline().await?;
    let pipeline = pipeline.with_hook_manager(visibility.hook_manager());
    pipeline
        .seed([
            Message::user().with_text("visibility"),
            Message::assistant()
                .with_text("visible first ")
                .with_id("visible-first"),
            Message::assistant()
                .with_text("internal message ")
                .agent_only()
                .with_id("internal"),
            Message::assistant()
                .with_content(MessageContent::Text(
                    TextContent::new("assistant-only block ").with_annotations(
                        Annotations::default().with_audience(vec![Role::Assistant]),
                    ),
                ))
                .with_content(MessageContent::Text(
                    TextContent::new("visible last")
                        .with_annotations(Annotations::default().with_audience(vec![Role::User])),
                ))
                .with_id("visible-last"),
        ])
        .await?;

    pipeline.resume().await?;
    assert_eq!(
        visibility.payloads()[0]["last_assistant_message"],
        "visible first visible last"
    );

    Ok(())
}

#[tokio::test]
async fn stop_hook_distinct_id_denials_retry_once_then_respect_block_cap() -> Result<()> {
    let blocked = HookTestEnv::new("Stop", RECORD_AND_BLOCK_MARKER_SCRIPT);
    let (pipeline, api) = test_pipeline().await?;
    let pipeline = pipeline
        .with_hook_manager(blocked.hook_manager())
        .with_stop_hook_block_cap(1);
    api.on("hello")
        .reply_with_distinct_ids(["policy marker: initial; ", "tail"]);
    api.on("blocked ending this turn")
        .reply_with_distinct_ids(["policy marker: retry; ", "tail"]);

    let (_, result, _) = pipeline.run_reconstructing_each_step("hello").await?;
    assert_eq!(api.call_count(), 2);
    assert_eq!(blocked.invocations(), 2);
    let payloads = blocked.payloads();
    assert_eq!(
        payloads[0]["last_assistant_message"],
        "policy marker: initial; tail"
    );
    assert_eq!(
        payloads[1]["last_assistant_message"],
        "policy marker: retry; tail"
    );
    assert_eq!(
        result
            .conversation()
            .messages()
            .iter()
            .filter(|message| message
                .metadata
                .operation_note("stop_hook", DENIED)
                .is_some())
            .count(),
        1
    );
    assert!(result.conversation().last().is_some_and(|message| {
        message.content.iter().any(|content| {
            matches!(
                content,
                MessageContent::SystemNotification(notification)
                    if notification.notification_type == SystemNotificationType::InlineMessage
                        && notification.msg.contains("GOOSE_STOP_HOOK_BLOCK_CAP")
            )
        })
    }));

    Ok(())
}

#[tokio::test]
async fn session_prompt_and_tool_hooks_fire_at_their_boundaries() -> Result<()> {
    let session_start = HookTestEnv::new("SessionStart", LOG_AND_ALLOW_SCRIPT);
    let (pipeline, api) = test_pipeline().await?;
    let pipeline = pipeline.with_hook_manager(session_start.hook_manager());
    api.on("first").reply("ok");
    api.on("second").reply("ok");

    pipeline.run(["/status"]).await?;
    assert_eq!(session_start.invocations(), 1);
    pipeline.run(["first", "second"]).await?;
    assert_eq!(session_start.invocations(), 1);
    assert_eq!(api.call_count(), 2);

    let prompt_submit = HookTestEnv::new("UserPromptSubmit", LOG_AND_ALLOW_SCRIPT);
    let (pipeline, api) = test_pipeline().await?;
    let pipeline = pipeline.with_hook_manager(prompt_submit.hook_manager());
    api.on("add one").call(ADD, value(1));
    api.on("result: 1").reply("done");

    pipeline.run(["/status"]).await?;
    assert_eq!(prompt_submit.invocations(), 1);
    let (_, result, _) = pipeline.run_reconstructing_each_step("add one").await?;
    result.assert_message(-1, Agent, "done");
    assert_eq!(api.call_count(), 2);
    assert_eq!(prompt_submit.invocations(), 2);

    let pre_tool = HookTestEnv::new("PreToolUse", LOG_AND_BLOCK_SCRIPT);
    let (pipeline, api) = test_pipeline().await?;
    let pipeline = pipeline.with_hook_manager(pre_tool.hook_manager());
    api.on("add one").call(ADD, value(1));
    api.on("denied by policy hook").reply("understood");

    let result = pipeline.run(["add one"]).await?;
    result.assert_message(-2, ToolResponse, "denied by policy hook");
    result.assert_message(-1, Agent, "understood");
    assert_eq!(pre_tool.invocations(), 1);
    assert_eq!(pipeline.calculator_total(), 0);

    Ok(())
}

/// Plugin fixture that can register several events at once, each with its own
/// matcher and script, and read back the JSON payloads a script recorded.
struct RecordingHookEnv {
    _temp_dir: tempfile::TempDir,
    plugin_dir: std::path::PathBuf,
}

/// (event name, matcher or "" for none, script file name, script body)
type HookSpec<'a> = (&'a str, &'a str, &'a str, &'a str);

impl RecordingHookEnv {
    fn new(specs: &[HookSpec<'_>]) -> Self {
        Self::with_on_failure(specs, "")
    }

    fn blocking_on_failure(specs: &[HookSpec<'_>]) -> Self {
        Self::with_on_failure(specs, r#", "on_failure": "block""#)
    }

    fn with_on_failure(specs: &[HookSpec<'_>], on_failure: &str) -> Self {
        let temp_dir = tempfile::tempdir().unwrap();
        let plugin_dir = temp_dir.path().join("test-plugin");
        std::fs::create_dir_all(plugin_dir.join("hooks")).unwrap();
        let entries: Vec<String> = specs
            .iter()
            .map(|(event, matcher, script, _)| {
                let matcher = if matcher.is_empty() {
                    String::new()
                } else {
                    format!(r#""matcher": "{matcher}", "#)
                };
                format!(
                    r#""{event}": [{{{matcher}"hooks": [{{"type": "command", "command": "sh ${{PLUGIN_ROOT}}/{script}"{on_failure}}}]}}]"#
                )
            })
            .collect();
        std::fs::write(
            plugin_dir.join("hooks/hooks.json"),
            format!(r#"{{"hooks": {{{}}}}}"#, entries.join(", ")),
        )
        .unwrap();
        for (_, _, script, body) in specs {
            std::fs::write(plugin_dir.join(script), body).unwrap();
        }
        Self {
            _temp_dir: temp_dir,
            plugin_dir,
        }
    }

    pub(super) fn hook_manager(&self) -> crate::hooks::HookManager {
        use crate::plugins::discovery::{DiscoveredPlugin, PluginScope};
        crate::hooks::HookManager::from_plugins_for_test(vec![DiscoveredPlugin {
            name: "test-plugin".into(),
            root: self.plugin_dir.clone(),
            scope: PluginScope::Project,
        }])
    }

    fn payloads(&self, log: &str) -> Vec<serde_json::Value> {
        std::fs::read_to_string(self.plugin_dir.join(log))
            .unwrap_or_default()
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}

const RECORD_PRE_SCRIPT: &str =
    "#!/bin/sh\ncat >> \"$PLUGIN_ROOT/pre.log\"\nprintf '\\n' >> \"$PLUGIN_ROOT/pre.log\"\nexit 0\n";
const RECORD_RESULT_SCRIPT: &str =
    "#!/bin/sh\ncat >> \"$PLUGIN_ROOT/result.log\"\nprintf '\\n' >> \"$PLUGIN_ROOT/result.log\"\nexit 0\n";
const RECORD_POST_SCRIPT: &str =
    "#!/bin/sh\ncat >> \"$PLUGIN_ROOT/post.log\"\nprintf '\\n' >> \"$PLUGIN_ROOT/post.log\"\nexit 0\n";
const RECORD_POST_FAILURE_SCRIPT: &str =
    "#!/bin/sh\ncat >> \"$PLUGIN_ROOT/postfail.log\"\nprintf '\\n' >> \"$PLUGIN_ROOT/postfail.log\"\nexit 0\n";
const RECORD_EXTENDED_SCRIPT: &str =
    "#!/bin/sh\ncat >> \"$PLUGIN_ROOT/extended.log\"\nprintf '\\n' >> \"$PLUGIN_ROOT/extended.log\"\nexit 0\n";
const DENY_AND_RECORD_SCRIPT: &str =
    "#!/bin/sh\ncat >> \"$PLUGIN_ROOT/pre.log\"\nprintf '\\n' >> \"$PLUGIN_ROOT/pre.log\"\necho \"blocked by test policy\" >&2\nexit 2\n";
/// Logs its stdin like the others, writes nothing to stdout, and exits
/// non-zero. That is a hook that ran but never returned a decision.
const ABNORMAL_EXIT_AND_RECORD_SCRIPT: &str =
    "#!/bin/sh\ncat >> \"$PLUGIN_ROOT/pre.log\"\nprintf '\\n' >> \"$PLUGIN_ROOT/pre.log\"\necho boom >&2\nexit 3\n";
const HOOK_FAILURE_REFUSAL: &str =
    "Tool call blocked because policy hook `test-plugin` could not complete: \
     the hook exited with status 3 and no usable decision. \
     That hook is configured to block on failure.";

/// deny-invisible: the tool never dispatches, neither post event fires, and a
/// PreToolUseResult subscriber still sees the denial with blocked_by and reason.
#[tokio::test]
async fn pre_tool_use_result_observes_denial_that_post_hooks_never_see() -> Result<()> {
    let env = RecordingHookEnv::new(&[
        ("PreToolUse", "", "pre.sh", DENY_AND_RECORD_SCRIPT),
        ("PreToolUseResult", "", "result.sh", RECORD_RESULT_SCRIPT),
        ("PostToolUse", "", "post.sh", RECORD_POST_SCRIPT),
        (
            "PostToolUseFailure",
            "",
            "postfail.sh",
            RECORD_POST_FAILURE_SCRIPT,
        ),
    ]);
    let (pipeline, api) = test_pipeline().await?;
    let pipeline = pipeline.with_hook_manager(env.hook_manager());
    api.on("add one").call(ADD, value(1));
    api.on("denied by policy hook").reply("understood");

    pipeline.run(["add one"]).await?;

    assert_eq!(pipeline.calculator_total(), 0, "tool must not dispatch");
    assert!(
        env.payloads("post.log").is_empty(),
        "PostToolUse must not fire for a denied call"
    );
    assert!(
        env.payloads("postfail.log").is_empty(),
        "PostToolUseFailure must not fire for a denied call"
    );

    let results = env.payloads("result.log");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["event"], "PreToolUseResult");
    assert_eq!(results[0]["decision"], "deny");
    assert_eq!(results[0]["policy_evaluated"], true);
    assert_eq!(results[0]["blocked_by"], "test-plugin");
    assert_eq!(results[0]["reason"], "blocked by test policy");
    assert!(results[0]["tool_call_id"]
        .as_str()
        .is_some_and(|id| !id.is_empty()));
    Ok(())
}

/// repeated identical calls: two calls with the same name and input in one
/// session correlate to their outcomes by tool_call_id, not by name plus input.
#[tokio::test]
async fn repeated_identical_calls_correlate_by_tool_call_id() -> Result<()> {
    let env = RecordingHookEnv::new(&[
        ("PreToolUse", "", "pre.sh", RECORD_PRE_SCRIPT),
        ("PreToolUseResult", "", "result.sh", RECORD_RESULT_SCRIPT),
        ("PostToolUse", "", "post.sh", RECORD_POST_SCRIPT),
    ]);
    let (pipeline, api) = test_pipeline().await?;
    let pipeline = pipeline.with_hook_manager(env.hook_manager());
    api.on("add one").call(ADD, value(1));
    api.on("result: 1").call(ADD, value(1));
    api.on("result: 2").reply("done");

    pipeline.run(["add one"]).await?;
    assert_eq!(pipeline.calculator_total(), 2);

    let pres = env.payloads("pre.log");
    let results = env.payloads("result.log");
    let posts = env.payloads("post.log");
    assert_eq!(pres.len(), 2);
    assert_eq!(results.len(), 2);
    assert_eq!(posts.len(), 2);

    for payloads in [&pres, &results, &posts] {
        assert_eq!(payloads[0]["tool_name"], payloads[1]["tool_name"]);
        assert_eq!(payloads[0]["tool_input"], payloads[1]["tool_input"]);
    }

    let ids: Vec<&str> = results
        .iter()
        .map(|payload| payload["tool_call_id"].as_str().unwrap())
        .collect();
    assert_ne!(
        ids[0], ids[1],
        "identical name and input must still carry distinct ids"
    );

    for (index, id) in ids.iter().enumerate() {
        assert_eq!(
            pres[index]["tool_call_id"], results[index]["tool_call_id"],
            "PreToolUse and PreToolUseResult must carry one id per call"
        );
        assert_eq!(
            posts
                .iter()
                .filter(|payload| payload["tool_call_id"] == *id)
                .count(),
            1,
            "each call must pair with exactly one outcome by id"
        );
    }
    Ok(())
}

/// no matching hook: a PreToolUse rule is registered but its matcher does not
/// match, so nothing runs and the event reports allow with policy_evaluated false.
#[tokio::test]
async fn pre_tool_use_result_reports_allow_and_unevaluated_when_no_hook_matches() -> Result<()> {
    let env = RecordingHookEnv::new(&[
        (
            "PreToolUse",
            "a_tool_name_that_never_matches",
            "pre.sh",
            DENY_AND_RECORD_SCRIPT,
        ),
        ("PreToolUseResult", "", "result.sh", RECORD_RESULT_SCRIPT),
    ]);
    let (pipeline, api) = test_pipeline().await?;
    let pipeline = pipeline.with_hook_manager(env.hook_manager());
    api.on("add one").call(ADD, value(1));
    api.on("result: 1").reply("done");

    pipeline.run(["add one"]).await?;
    assert_eq!(pipeline.calculator_total(), 1, "tool must still run");

    assert!(
        env.payloads("pre.log").is_empty(),
        "the non-matching rule must not run"
    );
    let results = env.payloads("result.log");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["decision"], "allow");
    assert_eq!(results[0]["policy_evaluated"], false);
    assert!(results[0].get("blocked_by").is_none());
    assert!(results[0].get("reason").is_none());
    Ok(())
}

/// sole abnormal hook: the only matching PreToolUse hook runs, writes nothing to
/// stdout and exits non-zero, so it never returned a decision. Execution stays
/// fail-open and the event reports allow with policy_evaluated false.
#[tokio::test]
async fn pre_tool_use_result_reports_unevaluated_when_the_only_hook_exits_without_a_decision(
) -> Result<()> {
    let env = RecordingHookEnv::new(&[
        ("PreToolUse", "", "pre.sh", ABNORMAL_EXIT_AND_RECORD_SCRIPT),
        ("PreToolUseResult", "", "result.sh", RECORD_RESULT_SCRIPT),
    ]);
    let (pipeline, api) = test_pipeline().await?;
    let pipeline = pipeline.with_hook_manager(env.hook_manager());
    api.on("add one").call(ADD, value(1));
    api.on("result: 1").reply("done");

    pipeline.run(["add one"]).await?;
    assert_eq!(pipeline.calculator_total(), 1, "tool must still run");

    assert_eq!(
        env.payloads("pre.log").len(),
        1,
        "the matching hook must still run"
    );
    let results = env.payloads("result.log");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["decision"], "allow");
    assert_eq!(results[0]["policy_evaluated"], false);
    // The pipeline mints the id rather than the caller, so pin that one is
    // present and non-empty instead of pinning a literal.
    assert!(
        results[0]["tool_call_id"]
            .as_str()
            .is_some_and(|id| !id.is_empty()),
        "the result event must carry the tool_call_id"
    );
    Ok(())
}

/// Builds a recipe whose structured response forces the model through
/// `recipe__final_output`, which `RecipeOperation` executes itself rather than
/// handing to `ToolExecutionOperation`.
fn final_output_recipe() -> crate::recipe::Recipe {
    crate::recipe::Recipe::builder()
        .title("Hook parity recipe")
        .description("Exercises the final-output hook lifecycle")
        .instructions("Return a structured answer")
        .response(crate::recipe::Response {
            json_schema: Some(serde_json::json!({
                "type": "object",
                "properties": { "answer": { "type": "string" } },
                "required": ["answer"]
            })),
        })
        .build()
        .expect("valid recipe")
}

/// recipe final-output parity: the call `RecipeOperation` executes directly still
/// emits `PreToolUse` and `PreToolUseResult`, correlated by one `tool_call_id`.
#[tokio::test]
async fn recipe_final_output_emits_pre_tool_use_and_result_with_matching_id() -> Result<()> {
    let env = RecordingHookEnv::new(&[
        ("PreToolUse", "", "pre.sh", RECORD_PRE_SCRIPT),
        ("PreToolUseResult", "", "result.sh", RECORD_RESULT_SCRIPT),
    ]);
    let (pipeline, api) = test_pipeline().await?;
    let pipeline = pipeline.with_hook_manager(env.hook_manager());
    pipeline.set_recipe(final_output_recipe()).await?;
    api.on("produce the answer").call(
        FINAL_OUTPUT_TOOL_NAME,
        serde_json::json!({ "answer": "done" }),
    );

    pipeline.run(["produce the answer"]).await?;

    let pres = env.payloads("pre.log");
    let results = env.payloads("result.log");
    assert_eq!(
        pres.len(),
        1,
        "PreToolUse must fire for recipe final output"
    );
    assert_eq!(
        results.len(),
        1,
        "PreToolUseResult must fire for recipe final output"
    );
    assert_eq!(pres[0]["tool_name"], FINAL_OUTPUT_TOOL_NAME);
    assert_eq!(results[0]["tool_name"], FINAL_OUTPUT_TOOL_NAME);
    assert_eq!(results[0]["event"], "PreToolUseResult");
    assert_eq!(results[0]["decision"], "allow");
    assert_eq!(
        pres[0]["tool_call_id"], results[0]["tool_call_id"],
        "PreToolUse and PreToolUseResult must carry the same tool_call_id"
    );
    assert!(pres[0]["tool_call_id"]
        .as_str()
        .is_some_and(|id| !id.is_empty()));
    Ok(())
}

/// recipe final-output parity: the post-tool event fires once the call completes,
/// carrying the id the pre events carried.
#[tokio::test]
async fn recipe_final_output_emits_post_tool_event() -> Result<()> {
    let env = RecordingHookEnv::new(&[
        ("PreToolUse", "", "pre.sh", RECORD_PRE_SCRIPT),
        ("PostToolUse", "", "post.sh", RECORD_POST_SCRIPT),
        (
            "PostToolUseFailure",
            "",
            "postfail.sh",
            RECORD_POST_FAILURE_SCRIPT,
        ),
    ]);
    let (pipeline, api) = test_pipeline().await?;
    let pipeline = pipeline.with_hook_manager(env.hook_manager());
    pipeline.set_recipe(final_output_recipe()).await?;
    api.on("produce the answer").call(
        FINAL_OUTPUT_TOOL_NAME,
        serde_json::json!({ "answer": "done" }),
    );

    pipeline.run(["produce the answer"]).await?;

    let pres = env.payloads("pre.log");
    let posts = env.payloads("post.log");
    assert_eq!(pres.len(), 1);
    assert_eq!(
        posts.len(),
        1,
        "PostToolUse must fire for a successful recipe final output"
    );
    assert_eq!(posts[0]["event"], "PostToolUse");
    assert_eq!(posts[0]["tool_name"], FINAL_OUTPUT_TOOL_NAME);
    assert_eq!(
        posts[0]["tool_call_id"], pres[0]["tool_call_id"],
        "the post event must carry the same tool_call_id as the pre events"
    );
    Ok(())
}

#[tokio::test]
async fn recipe_final_output_waits_for_sibling_tools_and_is_emitted_once() -> Result<()> {
    let (pipeline, api) = test_pipeline().await?;
    pipeline.set_recipe(final_output_recipe()).await?;
    api.on("finish and add").calls([
        (
            "final-output",
            FINAL_OUTPUT_TOOL_NAME,
            serde_json::json!({ "answer": "done" }),
        ),
        ("side-effect", ADD, value(1)),
    ]);

    let result = pipeline.run(["finish and add"]).await?;

    assert_eq!(pipeline.calculator_total(), 1);
    let messages = result.conversation().messages();
    let side_effect_response = messages
        .iter()
        .position(|message| {
            message.content.iter().any(|content| {
                matches!(
                    content,
                    MessageContent::ToolResponse(response) if response.id == "side-effect"
                )
            })
        })
        .expect("sibling tool response");
    let final_answers: Vec<_> = messages
        .iter()
        .enumerate()
        .filter(|(_, message)| message.as_concat_text() == r#"{"answer":"done"}"#)
        .collect();
    assert_eq!(final_answers.len(), 1, "final output must be emitted once");
    assert!(
        side_effect_response < final_answers[0].0,
        "final output must wait until sibling tools finish"
    );
    Ok(())
}

#[tokio::test]
async fn recipe_final_output_waits_for_approval_pending_sibling() -> Result<()> {
    let (pipeline, api) = test_pipeline().await?;
    let pipeline = pipeline.with_goose_mode(GooseMode::Approve).await;
    pipeline.set_permission(FINAL_OUTPUT_TOOL_NAME, PermissionLevel::AlwaysAllow);
    pipeline.set_recipe(final_output_recipe()).await?;
    api.on("finish after approval").calls([
        (
            "final-output",
            FINAL_OUTPUT_TOOL_NAME,
            serde_json::json!({ "answer": "approved" }),
        ),
        (
            "side-effect",
            MANAGE_EXTENSIONS_TOOL_NAME_COMPLETE,
            serde_json::json!({
                "action": "enable",
                "extension_name": "analyze"
            }),
        ),
    ]);

    let awaiting_approval = pipeline.run(["finish after approval"]).await?;
    assert!(
        awaiting_approval
            .conversation()
            .messages()
            .iter()
            .all(|message| message.as_concat_text() != r#"{"answer":"approved"}"#),
        "final output must wait while a sibling needs approval"
    );

    pipeline
        .confirm("side-effect", Permission::AllowOnce)
        .await?;
    let result = pipeline.resume().await?;

    let messages = result.conversation().messages();
    let sibling_response = messages
        .iter()
        .position(|message| {
            message.content.iter().any(|content| {
                matches!(
                    content,
                    MessageContent::ToolResponse(response) if response.id == "side-effect"
                )
            })
        })
        .expect("approved sibling response");
    let final_answers = result
        .conversation()
        .messages()
        .iter()
        .filter(|message| message.as_concat_text() == r#"{"answer":"approved"}"#)
        .count();
    assert_eq!(final_answers, 1, "approved final output must emit once");
    let final_answer = messages
        .iter()
        .position(|message| message.as_concat_text() == r#"{"answer":"approved"}"#)
        .expect("approved final output");
    assert!(
        sibling_response < final_answer,
        "the approved sibling must execute before finalization"
    );
    result.assert_message(-1, Agent, r#"{"answer":"approved"}"#);
    Ok(())
}

/// recipe final-output parity: a denying hook stops the call. The final-output
/// tool never runs, so the recipe never reports a successful structured answer,
/// and no post event fires — the same shape a denied ordinary tool call has.
#[tokio::test]
async fn recipe_final_output_denied_by_hook_does_not_execute() -> Result<()> {
    let env = RecordingHookEnv::new(&[
        ("PreToolUse", "", "pre.sh", DENY_AND_RECORD_SCRIPT),
        ("PreToolUseResult", "", "result.sh", RECORD_RESULT_SCRIPT),
        ("PostToolUse", "", "post.sh", RECORD_POST_SCRIPT),
        (
            "PostToolUseFailure",
            "",
            "postfail.sh",
            RECORD_POST_FAILURE_SCRIPT,
        ),
    ]);
    let (pipeline, api) = test_pipeline().await?;
    let pipeline = pipeline
        .with_hook_manager(env.hook_manager())
        .with_max_turns(2);
    pipeline.set_recipe(final_output_recipe()).await?;
    api.on("produce the answer").call(
        FINAL_OUTPUT_TOOL_NAME,
        serde_json::json!({ "answer": "done" }),
    );
    api.on("denied by policy hook").reply("understood");

    let result = pipeline.run(["produce the answer"]).await?;

    let results = env.payloads("result.log");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["decision"], "deny");
    assert_eq!(results[0]["blocked_by"], "test-plugin");
    assert_eq!(results[0]["tool_name"], FINAL_OUTPUT_TOOL_NAME);

    assert!(
        env.payloads("post.log").is_empty(),
        "PostToolUse must not fire for a denied final-output call"
    );
    assert!(
        env.payloads("postfail.log").is_empty(),
        "PostToolUseFailure must not fire for a denied final-output call"
    );

    let produced_answer = result
        .conversation()
        .messages()
        .iter()
        .any(|message| message.as_concat_text().contains("\"answer\""));
    assert!(
        !produced_answer,
        "a denied final-output call must not execute the tool"
    );
    Ok(())
}

/// Writes a skill `SkillOperation` can load, and returns the tool arguments that
/// load it. `load_skill` is executed by `SkillOperation`, which is registered
/// ahead of `ToolExecutionOperation`, so it never reaches the hook wrapper.
fn install_skill(working_dir: &std::path::Path) -> serde_json::Value {
    let skill_dir = working_dir.join(".agents/skills/review");
    std::fs::create_dir_all(&skill_dir).expect("skill dir");
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: review\ndescription: Review helper\n---\nSKILL_BODY_CONTENT\n",
    )
    .expect("skill file");
    serde_json::json!({ "name": "review" })
}

/// load_skill parity: the call `SkillOperation` executes directly still emits
/// `PreToolUse` and `PreToolUseResult`, correlated by one `tool_call_id`.
#[tokio::test]
async fn load_skill_emits_pre_tool_use_and_result_with_matching_id() -> Result<()> {
    let env = RecordingHookEnv::new(&[
        ("PreToolUse", "", "pre.sh", RECORD_PRE_SCRIPT),
        ("PreToolUseResult", "", "result.sh", RECORD_RESULT_SCRIPT),
    ]);
    let (pipeline, api) = test_pipeline().await?;
    let pipeline = pipeline.with_hook_manager(env.hook_manager());
    let arguments = install_skill(pipeline.working_dir());
    api.on("use the skill").call("load_skill", arguments);
    api.on("SKILL_BODY_CONTENT").reply("skill loaded");

    pipeline.run(["use the skill"]).await?;

    let pres = env.payloads("pre.log");
    let results = env.payloads("result.log");
    assert_eq!(pres.len(), 1, "PreToolUse must fire for load_skill");
    assert_eq!(
        results.len(),
        1,
        "PreToolUseResult must fire for load_skill"
    );
    assert_eq!(pres[0]["tool_name"], "load_skill");
    assert_eq!(results[0]["tool_name"], "load_skill");
    assert_eq!(results[0]["event"], "PreToolUseResult");
    assert_eq!(results[0]["decision"], "allow");
    assert_eq!(
        pres[0]["tool_call_id"], results[0]["tool_call_id"],
        "PreToolUse and PreToolUseResult must carry the same tool_call_id"
    );
    assert!(pres[0]["tool_call_id"]
        .as_str()
        .is_some_and(|id| !id.is_empty()));
    Ok(())
}

/// load_skill parity: the post-tool event fires once the skill load completes,
/// carrying the id the pre events carried.
#[tokio::test]
async fn load_skill_emits_post_tool_event() -> Result<()> {
    let env = RecordingHookEnv::new(&[
        ("PreToolUse", "", "pre.sh", RECORD_PRE_SCRIPT),
        ("PostToolUse", "", "post.sh", RECORD_POST_SCRIPT),
        (
            "PostToolUseFailure",
            "",
            "postfail.sh",
            RECORD_POST_FAILURE_SCRIPT,
        ),
    ]);
    let (pipeline, api) = test_pipeline().await?;
    let pipeline = pipeline.with_hook_manager(env.hook_manager());
    let arguments = install_skill(pipeline.working_dir());
    api.on("use the skill").call("load_skill", arguments);
    api.on("SKILL_BODY_CONTENT").reply("skill loaded");

    pipeline.run(["use the skill"]).await?;

    let pres = env.payloads("pre.log");
    let posts = env.payloads("post.log");
    assert_eq!(pres.len(), 1);
    assert_eq!(
        posts.len(),
        1,
        "PostToolUse must fire for a successful load_skill"
    );
    assert_eq!(posts[0]["event"], "PostToolUse");
    assert_eq!(posts[0]["tool_name"], "load_skill");
    assert_eq!(
        posts[0]["tool_call_id"], pres[0]["tool_call_id"],
        "the post event must carry the same tool_call_id as the pre events"
    );
    Ok(())
}

/// load_skill parity: a denying hook stops the call. The skill body never
/// reaches the conversation and no post event fires — the same shape a denied
/// ordinary tool call has.
#[tokio::test]
async fn load_skill_denied_by_hook_does_not_execute() -> Result<()> {
    let env = RecordingHookEnv::new(&[
        ("PreToolUse", "", "pre.sh", DENY_AND_RECORD_SCRIPT),
        ("PreToolUseResult", "", "result.sh", RECORD_RESULT_SCRIPT),
        ("PostToolUse", "", "post.sh", RECORD_POST_SCRIPT),
        (
            "PostToolUseFailure",
            "",
            "postfail.sh",
            RECORD_POST_FAILURE_SCRIPT,
        ),
    ]);
    let (pipeline, api) = test_pipeline().await?;
    let pipeline = pipeline.with_hook_manager(env.hook_manager());
    let arguments = install_skill(pipeline.working_dir());
    api.on("use the skill").call("load_skill", arguments);
    api.on("denied by policy hook").reply("understood");

    let result = pipeline.run(["use the skill"]).await?;

    let results = env.payloads("result.log");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["decision"], "deny");
    assert_eq!(results[0]["blocked_by"], "test-plugin");
    assert_eq!(results[0]["tool_name"], "load_skill");

    assert!(
        env.payloads("post.log").is_empty(),
        "PostToolUse must not fire for a denied load_skill call"
    );
    assert!(
        env.payloads("postfail.log").is_empty(),
        "PostToolUseFailure must not fire for a denied load_skill call"
    );

    let loaded_body = result
        .conversation()
        .messages()
        .iter()
        .any(|message| message.as_concat_text().contains("SKILL_BODY_CONTENT"));
    assert!(
        !loaded_body,
        "a denied load_skill call must not execute the skill load"
    );
    Ok(())
}

#[tokio::test]
async fn unadvertised_unknown_tool_skips_pre_tool_hooks() -> Result<()> {
    let env = RecordingHookEnv::new(&[
        ("PreToolUse", "", "pre.sh", RECORD_PRE_SCRIPT),
        ("PreToolUseResult", "", "result.sh", RECORD_RESULT_SCRIPT),
    ]);
    let (pipeline, api) = test_pipeline().await?;
    let pipeline = pipeline.with_hook_manager(env.hook_manager());
    api.on("try the missing tool")
        .unadvertised_call("missing__tool", serde_json::json!({}));
    api.on("not available").reply("recovered");

    pipeline.run(["try the missing tool"]).await?;

    assert!(env.payloads("pre.log").is_empty());
    assert!(env.payloads("result.log").is_empty());
    Ok(())
}

#[tokio::test]
async fn unadvertised_unknown_tool_skips_post_tool_hooks() -> Result<()> {
    let env = RecordingHookEnv::new(&[
        ("PreToolUse", "", "pre.sh", RECORD_PRE_SCRIPT),
        ("PostToolUse", "", "post.sh", RECORD_POST_SCRIPT),
        (
            "PostToolUseFailure",
            "",
            "postfail.sh",
            RECORD_POST_FAILURE_SCRIPT,
        ),
    ]);
    let (pipeline, api) = test_pipeline().await?;
    let pipeline = pipeline.with_hook_manager(env.hook_manager());
    api.on("try the missing tool")
        .unadvertised_call("missing__tool", serde_json::json!({}));
    api.on("not available").reply("recovered");

    pipeline.run(["try the missing tool"]).await?;

    assert!(env.payloads("pre.log").is_empty());
    assert!(env.payloads("post.log").is_empty());
    assert!(env.payloads("postfail.log").is_empty());
    Ok(())
}

#[tokio::test]
async fn unadvertised_inactive_final_output_skips_hooks() -> Result<()> {
    let env = RecordingHookEnv::new(&[(
        "PostToolUseFailure",
        "",
        "postfail.sh",
        RECORD_POST_FAILURE_SCRIPT,
    )]);
    let (pipeline, api) = test_pipeline().await?;
    let pipeline = pipeline.with_hook_manager(env.hook_manager());
    api.on("call inactive final output").unadvertised_call(
        FINAL_OUTPUT_TOOL_NAME,
        serde_json::json!({ "answer": "unused" }),
    );
    api.on("not available").reply("inactive call handled");

    let result = pipeline.run(["call inactive final output"]).await?;

    result.assert_message(-2, ToolResponse, "not available");
    assert!(env.payloads("postfail.log").is_empty());
    let response_metadata = result
        .conversation()
        .messages()
        .iter()
        .flat_map(|message| &message.content)
        .find_map(|content| match content {
            MessageContent::ToolResponse(response) => response.metadata.as_ref(),
            _ => None,
        });
    assert!(response_metadata.is_some_and(|metadata| {
        metadata
            .get(UNCLAIMED_TOOL_ERROR)
            .and_then(serde_json::Value::as_bool)
            == Some(true)
    }));
    Ok(())
}

#[tokio::test]
async fn unadvertised_shell_and_read_tools_skip_extended_pre_hooks() -> Result<()> {
    let shell = RecordingHookEnv::new(&[(
        "BeforeShellExecution",
        "echo lifecycle",
        "extended.sh",
        RECORD_EXTENDED_SCRIPT,
    )]);
    let (pipeline, api) = test_pipeline().await?;
    let pipeline = pipeline.with_hook_manager(shell.hook_manager());
    api.on("probe unknown shell").unadvertised_call(
        "missing__shell",
        serde_json::json!({ "command": "echo lifecycle" }),
    );
    api.on("not available").reply("shell probe complete");

    pipeline.run(["probe unknown shell"]).await?;

    assert!(shell.payloads("extended.log").is_empty());

    let read = RecordingHookEnv::new(&[(
        "BeforeReadFile",
        "/tmp/missing-lifecycle-file",
        "extended.sh",
        RECORD_EXTENDED_SCRIPT,
    )]);
    let (pipeline, api) = test_pipeline().await?;
    let pipeline = pipeline.with_hook_manager(read.hook_manager());
    api.on("probe unknown read").unadvertised_call(
        "missing__read",
        serde_json::json!({ "path": "/tmp/missing-lifecycle-file" }),
    );
    api.on("not available").reply("read probe complete");

    pipeline.run(["probe unknown read"]).await?;

    assert!(read.payloads("extended.log").is_empty());
    Ok(())
}

#[tokio::test]
async fn unadvertised_unknown_tool_is_not_intercepted_by_hooks() -> Result<()> {
    let env = RecordingHookEnv::new(&[
        ("PreToolUse", "", "pre.sh", DENY_AND_RECORD_SCRIPT),
        ("PreToolUseResult", "", "result.sh", RECORD_RESULT_SCRIPT),
        ("PostToolUse", "", "post.sh", RECORD_POST_SCRIPT),
        (
            "PostToolUseFailure",
            "",
            "postfail.sh",
            RECORD_POST_FAILURE_SCRIPT,
        ),
    ]);
    let (pipeline, api) = test_pipeline().await?;
    let pipeline = pipeline.with_hook_manager(env.hook_manager());
    api.on("try the missing tool")
        .unadvertised_call("missing__tool", serde_json::json!({}));
    api.on("not available").reply("understood");

    let result = pipeline.run(["try the missing tool"]).await?;

    assert!(env.payloads("pre.log").is_empty());
    assert!(env.payloads("result.log").is_empty());
    assert!(env.payloads("post.log").is_empty());
    assert!(env.payloads("postfail.log").is_empty());
    result.assert_message(-2, ToolResponse, "Tool 'missing__tool' is not available.");
    Ok(())
}

#[tokio::test]
async fn chat_mode_does_not_collect_skipped_recipe_final_output_or_run_hooks() -> Result<()> {
    let env = RecordingHookEnv::new(&[
        ("PreToolUse", "", "pre.sh", RECORD_PRE_SCRIPT),
        ("PreToolUseResult", "", "result.sh", RECORD_RESULT_SCRIPT),
        ("PostToolUse", "", "post.sh", RECORD_POST_SCRIPT),
        (
            "PostToolUseFailure",
            "",
            "postfail.sh",
            RECORD_POST_FAILURE_SCRIPT,
        ),
    ]);
    let (pipeline, api) = test_pipeline().await?;
    let pipeline = pipeline
        .with_hook_manager(env.hook_manager())
        .with_goose_mode(GooseMode::Chat)
        .await
        .with_max_turns(2);
    pipeline.set_recipe(final_output_recipe()).await?;
    api.on("produce the answer").call(
        FINAL_OUTPUT_TOOL_NAME,
        serde_json::json!({ "answer": "done" }),
    );
    api.on(CHAT_MODE_TOOL_SKIPPED_RESPONSE)
        .reply("continued without the final output tool");

    let result = pipeline.run(["produce the answer"]).await?;

    let messages = result.conversation().messages();
    let emitted_chat_skip = messages
        .iter()
        .flat_map(|message| &message.content)
        .any(|content| match content {
            MessageContent::ToolResponse(response) => {
                response.tool_result.as_ref().is_ok_and(|result| {
                    result.content.iter().any(|content| {
                        content
                            .as_text()
                            .is_some_and(|text| text.text == CHAT_MODE_TOOL_SKIPPED_RESPONSE)
                    })
                })
            }
            _ => false,
        });
    assert!(emitted_chat_skip, "Chat mode must emit its skip response");
    assert!(
        messages
            .iter()
            .all(|message| message.as_concat_text() != r#"{"answer":"done"}"#),
        "a skipped final-output call must not be collected"
    );
    assert!(env.payloads("pre.log").is_empty());
    assert!(env.payloads("result.log").is_empty());
    assert!(env.payloads("post.log").is_empty());
    assert!(env.payloads("postfail.log").is_empty());
    Ok(())
}

#[tokio::test]
async fn chat_mode_rejects_unadvertised_unknown_tool_without_hooks() -> Result<()> {
    let env = RecordingHookEnv::new(&[
        ("PreToolUse", "", "pre.sh", RECORD_PRE_SCRIPT),
        ("PreToolUseResult", "", "result.sh", RECORD_RESULT_SCRIPT),
        ("PostToolUse", "", "post.sh", RECORD_POST_SCRIPT),
        (
            "PostToolUseFailure",
            "",
            "postfail.sh",
            RECORD_POST_FAILURE_SCRIPT,
        ),
    ]);
    let (pipeline, api) = test_pipeline().await?;
    let pipeline = pipeline
        .with_hook_manager(env.hook_manager())
        .with_goose_mode(GooseMode::Chat)
        .await;
    api.on("try the missing tool")
        .unadvertised_call("missing__tool", serde_json::json!({}));
    api.on("not available").reply("hidden tool rejected");

    let result = pipeline.run(["try the missing tool"]).await?;

    result.assert_message(-2, ToolResponse, "Tool 'missing__tool' is not available.");
    result.assert_message(-1, Agent, "hidden tool rejected");
    assert!(env.payloads("pre.log").is_empty());
    assert!(env.payloads("result.log").is_empty());
    assert!(env.payloads("post.log").is_empty());
    assert!(env.payloads("postfail.log").is_empty());
    Ok(())
}

#[tokio::test]
async fn unadvertised_unknown_tool_skips_permission_and_tool_hooks() -> Result<()> {
    let env = RecordingHookEnv::new(&[
        ("PreToolUse", "", "pre.sh", RECORD_PRE_SCRIPT),
        ("PreToolUseResult", "", "result.sh", RECORD_RESULT_SCRIPT),
        ("PostToolUse", "", "post.sh", RECORD_POST_SCRIPT),
        (
            "PostToolUseFailure",
            "",
            "postfail.sh",
            RECORD_POST_FAILURE_SCRIPT,
        ),
    ]);
    let (pipeline, api) = test_pipeline().await?;
    let pipeline = pipeline
        .with_hook_manager(env.hook_manager())
        .with_goose_mode(GooseMode::Approve)
        .await;
    pipeline.set_permission("missing__tool", PermissionLevel::NeverAllow);
    api.on("try the missing tool")
        .unadvertised_call("missing__tool", serde_json::json!({}));
    api.on("not available").reply("understood");

    let result = pipeline.run(["try the missing tool"]).await?;

    result.assert_message(-2, ToolResponse, "Tool 'missing__tool' is not available.");
    assert!(env.payloads("pre.log").is_empty());
    assert!(env.payloads("result.log").is_empty());
    assert!(env.payloads("post.log").is_empty());
    assert!(env.payloads("postfail.log").is_empty());
    Ok(())
}

/// Enforces a bijection between tool requests and tool responses over the whole
/// transcript. Presence alone is not enough: a duplicate response reuses a
/// tool_call_id and an orphan response names one that was never requested, and
/// strict providers can reject either on the next request.
fn assert_tool_transcript_bijection(messages: &[Message]) {
    let mut request_ids: Vec<String> = Vec::new();
    let mut response_ids: Vec<String> = Vec::new();
    for message in messages {
        for content in &message.content {
            match content {
                MessageContent::ToolRequest(request) => request_ids.push(request.id.clone()),
                MessageContent::ToolResponse(response) => response_ids.push(response.id.clone()),
                _ => {}
            }
        }
    }
    let unique_requests: std::collections::HashSet<&String> = request_ids.iter().collect();
    assert_eq!(
        unique_requests.len(),
        request_ids.len(),
        "a tool request id appears more than once: {request_ids:?}"
    );
    for id in &request_ids {
        let answers = response_ids.iter().filter(|other| *other == id).count();
        assert_eq!(
            answers, 1,
            "request {id} has {answers} responses, expected exactly one; responses {response_ids:?}"
        );
    }
    for id in &response_ids {
        assert!(
            unique_requests.contains(id),
            "response {id} references no request; requests {request_ids:?}"
        );
    }
}

fn lifecycle_ids(env: &RecordingHookEnv, log: &str) -> Vec<String> {
    env.payloads(log)
        .iter()
        .filter_map(|payload| payload["tool_call_id"].as_str().map(str::to_string))
        .collect()
}

fn recording_lifecycle_env() -> RecordingHookEnv {
    RecordingHookEnv::new(&[
        ("PreToolUse", "", "pre.sh", RECORD_PRE_SCRIPT),
        ("PreToolUseResult", "", "result.sh", RECORD_RESULT_SCRIPT),
        ("PostToolUse", "", "post.sh", RECORD_POST_SCRIPT),
        (
            "PostToolUseFailure",
            "",
            "postfail.sh",
            RECORD_POST_FAILURE_SCRIPT,
        ),
    ])
}

/// A final-output call refused by permission is answered like any other declined
/// tool. RecipeOperation matches on Execute alone, so before the fix nothing
/// answered this request at all.
#[tokio::test]
async fn recipe_final_output_denied_by_permission_receives_declined_response() -> Result<()> {
    let env = recording_lifecycle_env();
    let (pipeline, api) = test_pipeline().await?;
    let pipeline = pipeline
        .with_hook_manager(env.hook_manager())
        .with_goose_mode(GooseMode::Approve)
        .await;
    pipeline.set_permission(FINAL_OUTPUT_TOOL_NAME, PermissionLevel::NeverAllow);
    pipeline.set_recipe(final_output_recipe()).await?;
    api.on("finish now").call(
        FINAL_OUTPUT_TOOL_NAME,
        serde_json::json!({ "answer": "denied" }),
    );
    api.on("declined to run this tool").reply("understood");

    let result = pipeline.run(["finish now"]).await?;
    let messages = result.conversation().messages();
    assert_tool_transcript_bijection(messages);

    let declined = messages
        .iter()
        .flat_map(|message| &message.content)
        .filter(|content| match content {
            MessageContent::ToolResponse(response) => {
                response.tool_result.as_ref().is_ok_and(|result| {
                    result.content.iter().any(|block| {
                        block
                            .as_text()
                            .is_some_and(|text| text.text == DECLINED_RESPONSE)
                    })
                })
            }
            _ => false,
        })
        .count();
    assert_eq!(
        declined, 1,
        "the declined call must get one DECLINED_RESPONSE"
    );

    assert!(
        env.payloads("pre.log").is_empty(),
        "no PreToolUse on decline"
    );
    assert!(
        env.payloads("result.log").is_empty(),
        "no PreToolUseResult on decline"
    );
    assert!(
        env.payloads("post.log").is_empty(),
        "no PostToolUse on decline"
    );
    assert!(
        env.payloads("postfail.log").is_empty(),
        "no PostToolUseFailure on decline"
    );
    Ok(())
}

/// Two final-output calls in one assistant block both get answered, each with its
/// own lifecycle, and the last valid one is published. Before the fix the pair
/// deadlocked: each waited for the other and neither was answered.
#[tokio::test]
async fn duplicate_final_output_calls_are_each_answered_and_last_valid_wins() -> Result<()> {
    let env = recording_lifecycle_env();
    let (pipeline, api) = test_pipeline().await?;
    let pipeline = pipeline.with_hook_manager(env.hook_manager());
    pipeline.set_recipe(final_output_recipe()).await?;
    api.on("finish twice").calls([
        (
            "final-a",
            FINAL_OUTPUT_TOOL_NAME,
            serde_json::json!({ "answer": "first" }),
        ),
        (
            "final-b",
            FINAL_OUTPUT_TOOL_NAME,
            serde_json::json!({ "answer": "second" }),
        ),
    ]);

    let result = pipeline.run(["finish twice"]).await?;
    let messages = result.conversation().messages();
    assert_tool_transcript_bijection(messages);

    // Assert the recorded order, not just membership: execution order is what
    // decides which call wins, so sorting here would erase the property.
    let pre = lifecycle_ids(&env, "pre.log");
    let results = lifecycle_ids(&env, "result.log");
    let posts = lifecycle_ids(&env, "post.log");
    assert!(
        env.payloads("postfail.log").is_empty(),
        "both calls succeed, so no post-failure event may be recorded"
    );
    let expected = vec!["final-a".to_string(), "final-b".to_string()];
    assert_eq!(pre, expected, "both calls need a PreToolUse");
    assert_eq!(results, expected, "both calls need a PreToolUseResult");
    assert_eq!(posts, expected, "both calls need a post event");

    let published: Vec<_> = messages
        .iter()
        .filter(|message| message.as_concat_text() == r#"{"answer":"second"}"#)
        .collect();
    assert_eq!(
        published.len(),
        1,
        "the last valid final output is published exactly once"
    );
    assert!(
        messages
            .iter()
            .all(|message| message.as_concat_text() != r#"{"answer":"first"}"#),
        "the superseded final output must not be published"
    );
    Ok(())
}

/// A malformed final-output call gets one parse-error response and runs no
/// lifecycle, because nothing executes.
#[tokio::test]
async fn malformed_final_output_call_receives_parse_error_and_no_lifecycle() -> Result<()> {
    let env = recording_lifecycle_env();
    let (pipeline, api) = test_pipeline().await?;
    let pipeline = pipeline
        .with_hook_manager(env.hook_manager())
        .with_max_turns(2);
    pipeline.set_recipe(final_output_recipe()).await?;
    api.on("finish badly")
        .malformed_tool_call(FINAL_OUTPUT_TOOL_NAME, r#"{"answer":"#);
    api.on("could not be parsed").reply("understood");

    let result = pipeline.run(["finish badly"]).await?;
    let messages = result.conversation().messages();
    assert_tool_transcript_bijection(messages);

    // The parse error rides on the tool response, not on message text, so
    // as_concat_text would not see it.
    let parse_errors = messages
        .iter()
        .flat_map(|message| &message.content)
        .filter(|content| match content {
            MessageContent::ToolResponse(response) => {
                response.tool_result.as_ref().is_ok_and(|result| {
                    result.content.iter().any(|block| {
                        block
                            .as_text()
                            .is_some_and(|text| text.text.contains("could not be parsed"))
                    })
                })
            }
            _ => false,
        })
        .count();
    assert_eq!(parse_errors, 1, "one parse-error response");

    assert!(env.payloads("pre.log").is_empty());
    assert!(env.payloads("result.log").is_empty());
    assert!(env.payloads("post.log").is_empty());
    assert!(env.payloads("postfail.log").is_empty());
    Ok(())
}

/// An unfinished ordinary sibling still delays publication, and every request in
/// the block is answered exactly once.
#[tokio::test]
async fn final_output_waits_for_ordinary_sibling_and_answers_every_request() -> Result<()> {
    let (pipeline, api) = test_pipeline().await?;
    pipeline.set_recipe(final_output_recipe()).await?;
    api.on("finish and add").calls([
        (
            "final-output",
            FINAL_OUTPUT_TOOL_NAME,
            serde_json::json!({ "answer": "after sibling" }),
        ),
        ("side-effect", ADD, value(1)),
    ]);

    let result = pipeline.run(["finish and add"]).await?;
    let messages = result.conversation().messages();
    assert_tool_transcript_bijection(messages);
    assert_eq!(pipeline.calculator_total(), 1);

    let sibling_answer = messages
        .iter()
        .position(|message| {
            message.content.iter().any(|content| {
                matches!(
                    content,
                    MessageContent::ToolResponse(response) if response.id == "side-effect"
                )
            })
        })
        .expect("sibling tool response");
    let published = messages
        .iter()
        .position(|message| message.as_concat_text() == r#"{"answer":"after sibling"}"#)
        .expect("published final output");
    assert!(
        sibling_answer < published,
        "final output must wait until the ordinary sibling finishes"
    );
    Ok(())
}

#[tokio::test]
async fn pre_tool_use_hook_failure_allows_by_default() -> Result<()> {
    let env = RecordingHookEnv::new(&[
        ("PreToolUse", "", "pre.sh", ABNORMAL_EXIT_AND_RECORD_SCRIPT),
        ("PreToolUseResult", "", "result.sh", RECORD_RESULT_SCRIPT),
        ("PostToolUse", "", "post.sh", RECORD_POST_SCRIPT),
    ]);
    let (pipeline, api) = test_pipeline().await?;
    let pipeline = pipeline.with_hook_manager(env.hook_manager());
    api.on("add one").call(ADD, value(1));
    api.on("result: 1").reply("done");

    pipeline.run(["add one"]).await?;

    assert_eq!(
        pipeline.calculator_total(),
        1,
        "a broken hook must not block"
    );
    assert_eq!(env.payloads("post.log").len(), 1);
    let results = env.payloads("result.log");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["decision"], "allow");
    assert_eq!(results[0]["cause"], "hook_failure");
    assert_eq!(results[0]["policy_evaluated"], false);
    Ok(())
}

#[tokio::test]
async fn pre_tool_use_hook_failure_blocks_when_configured() -> Result<()> {
    let env = RecordingHookEnv::blocking_on_failure(&[
        ("PreToolUse", "", "pre.sh", ABNORMAL_EXIT_AND_RECORD_SCRIPT),
        ("PreToolUseResult", "", "result.sh", RECORD_RESULT_SCRIPT),
        ("PostToolUse", "", "post.sh", RECORD_POST_SCRIPT),
    ]);
    let (pipeline, api) = test_pipeline().await?;
    let pipeline = pipeline.with_hook_manager(env.hook_manager());
    api.on("add one").call(ADD, value(1));
    api.on("could not complete").reply("understood");

    let result = pipeline.run(["add one"]).await?;

    assert_eq!(pipeline.calculator_total(), 0, "tool must not dispatch");
    assert!(
        env.payloads("post.log").is_empty(),
        "PostToolUse must not fire for a blocked call"
    );

    let results = env.payloads("result.log");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["decision"], "deny");
    assert_eq!(results[0]["cause"], "hook_failure");
    assert_eq!(results[0]["policy_evaluated"], false);
    assert_eq!(results[0]["blocked_by"], "test-plugin");

    let tool_error = result
        .conversation()
        .messages()
        .iter()
        .flat_map(|message| &message.content)
        .find_map(|content| match content {
            MessageContent::ToolResponse(response) => response.tool_result.as_ref().err(),
            _ => None,
        })
        .expect("blocked tool response");
    assert!(
        tool_error.message.ends_with(HOOK_FAILURE_REFUSAL),
        "expected the shared refusal, got {}",
        tool_error.message
    );
    Ok(())
}
