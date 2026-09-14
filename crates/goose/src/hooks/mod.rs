//! Lifecycle hooks support, modelled after the Open Plugins
//! [hooks specification](https://open-plugins.com/agent-builders/components/hooks).
//!
//! Hooks live in `<plugin-root>/hooks/hooks.json` of any plugin discovered by
//! [`crate::plugins::discovery::discover_enabled_plugins`]. The schema is:
//!
//! ```json
//! {
//!   "hooks": {
//!     "PostToolUse": [
//!       {
//!         "matcher": "developer__shell|developer__text_editor",
//!         "hooks": [
//!           { "type": "command", "command": "${PLUGIN_ROOT}/scripts/log.sh" }
//!         ]
//!       }
//!     ]
//!   }
//! }
//! ```
//!
//! Goose currently supports `type: "command"` actions. Unknown event names and
//! action types are ignored per the spec. Hook scripts receive the JSON event
//! context on stdin and SHOULD exit 0 on success.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tracing::{debug, info, warn};
use tracing_futures::Instrument;

use crate::plugins::discovery::{discover_enabled_plugins, DiscoveredPlugin};

/// Default per-hook timeout when the plugin does not specify one.
const DEFAULT_HOOK_TIMEOUT_SECS: u64 = 30;

const COMMAND_FAILED_REASON: &str = "the hook command failed to run";
const SERIALIZATION_FAILED_REASON: &str = "the hook payload could not be serialized";
const STDIN_DELIVERY_FAILED_REASON: &str = "the hook did not receive the request payload";

/// Lifecycle events a hook can subscribe to.
///
/// The variant names match the event names used in `hooks.json`. Unknown
/// events in user config are ignored at load time, per the spec.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HookEvent {
    PreToolUse,
    PreToolUseResult,
    PostToolUse,
    PostToolUseFailure,
    SessionStart,
    SessionEnd,
    UserPromptSubmit,
    BeforeReadFile,
    AfterFileEdit,
    BeforeShellExecution,
    AfterShellExecution,
    Stop,
}

impl HookEvent {
    fn name(&self) -> &'static str {
        match self {
            HookEvent::PreToolUse => "PreToolUse",
            HookEvent::PreToolUseResult => "PreToolUseResult",
            HookEvent::PostToolUse => "PostToolUse",
            HookEvent::PostToolUseFailure => "PostToolUseFailure",
            HookEvent::SessionStart => "SessionStart",
            HookEvent::SessionEnd => "SessionEnd",
            HookEvent::UserPromptSubmit => "UserPromptSubmit",
            HookEvent::BeforeReadFile => "BeforeReadFile",
            HookEvent::AfterFileEdit => "AfterFileEdit",
            HookEvent::BeforeShellExecution => "BeforeShellExecution",
            HookEvent::AfterShellExecution => "AfterShellExecution",
            HookEvent::Stop => "Stop",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "PreToolUse" => HookEvent::PreToolUse,
            "PreToolUseResult" => HookEvent::PreToolUseResult,
            "PostToolUse" => HookEvent::PostToolUse,
            "PostToolUseFailure" => HookEvent::PostToolUseFailure,
            "SessionStart" => HookEvent::SessionStart,
            "SessionEnd" => HookEvent::SessionEnd,
            "UserPromptSubmit" => HookEvent::UserPromptSubmit,
            "BeforeReadFile" => HookEvent::BeforeReadFile,
            "AfterFileEdit" => HookEvent::AfterFileEdit,
            "BeforeShellExecution" => HookEvent::BeforeShellExecution,
            "AfterShellExecution" => HookEvent::AfterShellExecution,
            "Stop" => HookEvent::Stop,
            _ => return None,
        })
    }
}

impl std::fmt::Display for HookEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

#[derive(Debug, Default, Deserialize)]
struct HooksFile {
    #[serde(default)]
    hooks: HashMap<String, Value>,
}

#[derive(Debug, Deserialize)]
struct RawHookRule {
    #[serde(default)]
    matcher: Option<String>,
    #[serde(default)]
    hooks: Vec<Value>,
}

#[derive(Debug, Deserialize)]
struct RawCommandAction {
    command: String,
    #[serde(default)]
    timeout: Option<u64>,
    #[serde(default, deserialize_with = "deserialize_present_on_failure")]
    on_failure: Option<Value>,
}

#[derive(Debug, PartialEq, Eq)]
enum ActionSelection {
    Selected,
    Ignored,
}

fn select_action(action: &Value, plugin_name: &str, path: &Path) -> Result<ActionSelection> {
    let Some(obj) = action.as_object() else {
        anyhow::bail!("hook action in {} must be an object", path.display());
    };

    match obj.get("type") {
        None | Some(Value::Null) => {}
        Some(Value::String(action_type)) if action_type == "command" => {}
        Some(Value::String(action_type)) => {
            debug!(
                plugin = plugin_name,
                action_type = %action_type,
                "Ignoring unsupported hook action type",
            );
            return Ok(ActionSelection::Ignored);
        }
        Some(_) => anyhow::bail!("hook action `type` in {} must be a string", path.display()),
    }

    match obj.get("command") {
        None | Some(Value::Null) => {
            debug!(
                plugin = plugin_name,
                "Ignoring command hook action with no command",
            );
            Ok(ActionSelection::Ignored)
        }
        Some(Value::String(_)) => Ok(ActionSelection::Selected),
        Some(_) => anyhow::bail!(
            "hook action `command` in {} must be a string",
            path.display()
        ),
    }
}

fn deserialize_present_on_failure<'de, D>(d: D) -> Result<Option<serde_json::Value>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    serde_json::Value::deserialize(d).map(Some)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum OnFailure {
    #[default]
    Allow,
    Block,
}

/// A loaded, plugin-bound hook rule ready to execute.
#[derive(Debug, Clone)]
struct LoadedRule {
    plugin_name: String,
    plugin_root: PathBuf,
    matcher: Option<Regex>,
    actions: Vec<LoadedAction>,
}

#[derive(Debug, Clone)]
enum LoadedAction {
    Command {
        command: String,
        timeout: Duration,
        on_failure: OnFailure,
    },
}

impl LoadedAction {
    fn on_failure(&self) -> OnFailure {
        let LoadedAction::Command { on_failure, .. } = self;
        *on_failure
    }
}

/// Context passed to a hook as JSON on stdin.
///
/// The `matcher_context` is the string the rule's `matcher` regex is tested
/// against — tool name for tool events, file path for file events, command
/// string for shell events. Other fields carry the same value plus the
/// raw JSON payload of the underlying event so scripts can do richer things
/// without needing to parse a hook-specific schema.
#[derive(Debug, Clone, Serialize)]
pub struct HookContext {
    pub event: String,
    pub session_id: String,
    pub matcher_context: Option<String>,
    /// Stable identifier for one tool call, the same value goose records as
    /// `gen_ai.tool.call.id`. Correlates the pre and post events of a single
    /// call, which tool name plus input cannot do when a call repeats.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_input: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_output: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_assistant_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<String>,
    /// `PreToolUseResult` only: "allow" or "deny". There is no third value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decision: Option<String>,
    /// `PreToolUseResult` only: true when at least one matching `PreToolUse`
    /// hook ran to completion for this call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy_evaluated: Option<bool>,
    /// `PreToolUseResult` on deny only: the plugin that denied.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked_by: Option<String>,
    /// `PreToolUseResult` on deny only: the reason the plugin gave.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl HookContext {
    pub fn new(event: HookEvent, session_id: impl Into<String>) -> Self {
        Self {
            event: event.to_string(),
            session_id: session_id.into(),
            matcher_context: None,
            tool_call_id: None,
            tool_name: None,
            tool_input: None,
            tool_output: None,
            message: None,
            last_assistant_message: None,
            working_dir: None,
            decision: None,
            policy_evaluated: None,
            blocked_by: None,
            reason: None,
        }
    }

    pub fn with_tool(mut self, tool_name: impl Into<String>, tool_input: Option<Value>) -> Self {
        let name = tool_name.into();
        self.matcher_context = Some(name.clone());
        self.tool_name = Some(name);
        self.tool_input = tool_input;
        self
    }

    pub fn with_tool_output(mut self, output: Value) -> Self {
        self.tool_output = Some(output);
        self
    }

    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        let msg = message.into();
        self.matcher_context.get_or_insert_with(|| msg.clone());
        self.message = Some(msg);
        self
    }

    pub fn with_last_assistant_message(mut self, message: impl Into<String>) -> Self {
        let message = message.into();
        if !message.is_empty() {
            self.last_assistant_message = Some(message);
        }
        self
    }

    pub fn with_working_dir(mut self, dir: impl Into<String>) -> Self {
        self.working_dir = Some(dir.into());
        self
    }

    pub fn with_tool_call_id(mut self, tool_call_id: impl Into<String>) -> Self {
        self.tool_call_id = Some(tool_call_id.into());
        self
    }

    /// Populate the `PreToolUseResult` outcome fields. `blocked_by` and `reason`
    /// are set only on deny, so an allow payload omits them entirely.
    pub(crate) fn with_pre_tool_use_outcome(
        mut self,
        outcome: &HookChainOutcome,
    ) -> PreToolUseResultPayload {
        self.policy_evaluated = Some(outcome.policy_evaluated);
        match &outcome.decision {
            HookDecision::Allow => self.decision = Some("allow".to_string()),
            HookDecision::Deny { reason, plugin } => {
                self.decision = Some("deny".to_string());
                self.blocked_by = Some(plugin.clone());
                self.reason = Some(reason.clone());
            }
        }
        PreToolUseResultPayload {
            context: self,
            cause: outcome.cause.map(|cause| cause.as_str().to_string()),
        }
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct PreToolUseResultPayload {
    #[serde(flatten)]
    context: HookContext,
    #[serde(skip_serializing_if = "Option::is_none")]
    cause: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookDecision {
    Allow,
    Deny { reason: String, plugin: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HookOutcomeCause {
    PolicyDenial,
    HookFailure,
}

impl HookOutcomeCause {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            HookOutcomeCause::PolicyDenial => "policy_denial",
            HookOutcomeCause::HookFailure => "hook_failure",
        }
    }
}

/// Crate-internal: the public [`HookManager::emit_blocking`] contract is
/// unchanged and still returns a [`HookDecision`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HookChainOutcome {
    pub decision: HookDecision,
    pub policy_evaluated: bool,
    pub cause: Option<HookOutcomeCause>,
}

impl HookChainOutcome {
    pub(crate) fn allow(policy_evaluated: bool) -> Self {
        Self {
            decision: HookDecision::Allow,
            policy_evaluated,
            cause: None,
        }
    }

    pub(crate) fn denial(&self) -> Option<HookDenial> {
        let HookDecision::Deny { reason, plugin } = &self.decision else {
            return None;
        };
        Some(match self.cause {
            Some(HookOutcomeCause::HookFailure) => HookDenial {
                message: format!(
                    "Tool call blocked because policy hook `{plugin}` could not complete: \
                     {reason}. That hook is configured to block on failure."
                ),
                error_type: "hook_failed",
            },
            _ => HookDenial {
                message: format!(
                    "Tool call denied by policy hook `{plugin}`: {reason}. \
                     Do not retry; this is a policy denial, not a transient failure."
                ),
                error_type: "hook_denied",
            },
        })
    }
}

pub(crate) struct HookDenial {
    pub message: String,
    pub error_type: &'static str,
}

/// Loads and executes plugin hooks.
#[derive(Debug, Default, Clone)]
pub struct HookManager {
    rules: HashMap<HookEvent, Vec<LoadedRule>>,
    use_login_shell_path: bool,
}

impl HookManager {
    /// Build a manager by scanning all enabled plugins for `hooks/hooks.json`.
    pub fn load(project_root: Option<&Path>, use_login_shell_path: bool) -> Self {
        let plugins = discover_enabled_plugins(project_root);
        Self::from_plugins(plugins, use_login_shell_path)
    }

    #[cfg(test)]
    pub(crate) fn from_plugins_for_test(plugins: Vec<DiscoveredPlugin>) -> Self {
        Self::from_plugins(plugins, false)
    }

    fn from_plugins(plugins: Vec<DiscoveredPlugin>, use_login_shell_path: bool) -> Self {
        let mut rules: HashMap<HookEvent, Vec<LoadedRule>> = HashMap::new();
        let mut total = 0usize;

        for plugin in plugins {
            let hooks_path = plugin.root.join("hooks").join("hooks.json");
            if !hooks_path.is_file() {
                continue;
            }
            match load_hooks_file(&hooks_path, &plugin.name, &plugin.root) {
                Ok(loaded) => {
                    for (event, plugin_rules) in loaded {
                        total += plugin_rules.len();
                        rules.entry(event).or_default().extend(plugin_rules);
                    }
                }
                Err(err) => warn!(
                    plugin = %plugin.name,
                    path = %hooks_path.display(),
                    error = %err,
                    "Failed to load plugin hooks; skipping",
                ),
            }
        }

        if total > 0 {
            info!(
                rule_count = total,
                events = ?rules.keys().map(|e| e.name()).collect::<Vec<_>>(),
                "Loaded plugin hooks",
            );
        }

        Self {
            rules,
            use_login_shell_path,
        }
    }

    /// Returns true if any rule is registered for `event`.
    pub fn has_hooks(&self, event: HookEvent) -> bool {
        self.rules.get(&event).is_some_and(|r| !r.is_empty())
    }

    async fn run_action(
        &self,
        event: HookEvent,
        session_id: &str,
        rule: &LoadedRule,
        command: &str,
        payload: &str,
        timeout: Duration,
    ) -> Result<HookRun> {
        let span = tracing::info_span!(
            target: "goose::hooks",
            "execute_hook",
            "gen_ai.operation.name" = "execute_hook",
            "goose.hook.event" = %event,
            "goose.hook.plugin" = %rule.plugin_name,
            "error.type" = tracing::field::Empty,
            session.id = %session_id,
        );
        let result = run_command_hook(
            command,
            &rule.plugin_root,
            payload,
            timeout,
            self.use_login_shell_path,
        )
        .instrument(span.clone())
        .await;
        match &result {
            Ok(run) if !run.output.status.success() => {
                span.record("error.type", "hook_exit");
            }
            Err(_) => {
                span.record("error.type", "hook_execution_error");
            }
            _ => {}
        }
        result
    }

    /// Fire all rules whose matcher matches the event context. Errors from
    /// individual hooks are logged but never propagated — a misbehaving hook
    /// MUST NOT crash the host tool.
    pub async fn emit(&self, event: HookEvent, ctx: HookContext) {
        if !self.has_hooks(event) {
            return;
        }
        let payload = match serde_json::to_string(&ctx) {
            Ok(s) => s,
            Err(err) => {
                warn!(event = %event, error = %err, "Failed to serialize hook context");
                return;
            }
        };
        self.emit_serialized(
            event,
            ctx.matcher_context.as_deref(),
            &ctx.session_id,
            &payload,
        )
        .await;
    }

    pub(crate) async fn emit_pre_tool_use_result(&self, payload: PreToolUseResultPayload) {
        let event = HookEvent::PreToolUseResult;
        if !self.has_hooks(event) {
            return;
        }
        let json = match serde_json::to_string(&payload) {
            Ok(s) => s,
            Err(err) => {
                warn!(event = %event, error = %err, "Failed to serialize hook context");
                return;
            }
        };
        self.emit_serialized(
            event,
            payload.context.matcher_context.as_deref(),
            &payload.context.session_id,
            &json,
        )
        .await;
    }

    async fn emit_serialized(
        &self,
        event: HookEvent,
        matcher_context: Option<&str>,
        session_id: &str,
        payload: &str,
    ) {
        let Some(rules) = self.rules.get(&event) else {
            return;
        };

        for rule in rules {
            if let Some(matcher) = &rule.matcher {
                let target = matcher_context.unwrap_or("");
                if !matcher.is_match(target) {
                    continue;
                }
            }

            for action in &rule.actions {
                let LoadedAction::Command {
                    command, timeout, ..
                } = action;
                debug!(
                    plugin = %rule.plugin_name,
                    event = %event,
                    command = %command,
                    "Running plugin hook",
                );
                let res = self
                    .run_action(event, session_id, rule, command, payload, *timeout)
                    .await
                    .and_then(|run| {
                        if run.output.status.success() {
                            Ok(())
                        } else {
                            anyhow::bail!(
                                "hook `{command}` exited with {:?}: {}",
                                run.output.status.code(),
                                String::from_utf8_lossy(&run.output.stderr).trim()
                            )
                        }
                    });
                if let Err(err) = res {
                    warn!(
                        plugin = %rule.plugin_name,
                        event = %event,
                        command = %command,
                        error = %err,
                        "Plugin hook failed",
                    );
                }
            }
        }
    }

    /// Like [`Self::emit`], but collects banner lines from hook stdout.
    ///
    /// If a hook exits successfully and its stdout contains valid JSON with a
    /// `"banner"` field, that string is collected. Multiple hooks can each
    /// contribute banner lines. Non-JSON stdout or missing `"banner"` field
    /// is silently ignored (backwards compatible).
    pub async fn emit_collecting_banners(&self, event: HookEvent, ctx: HookContext) -> Vec<String> {
        let mut banners = Vec::new();
        let Some(rules) = self.rules.get(&event) else {
            return banners;
        };
        if rules.is_empty() {
            return banners;
        }

        let payload = match serde_json::to_string(&ctx) {
            Ok(s) => s,
            Err(err) => {
                warn!(event = %event, error = %err, "Failed to serialize hook context");
                return banners;
            }
        };

        for rule in rules {
            if let Some(matcher) = &rule.matcher {
                let target = ctx.matcher_context.as_deref().unwrap_or("");
                if !matcher.is_match(target) {
                    continue;
                }
            }

            for action in &rule.actions {
                let LoadedAction::Command {
                    command, timeout, ..
                } = action;
                debug!(
                    plugin = %rule.plugin_name,
                    event = %event,
                    command = %command,
                    "Running plugin hook (banner-collecting)",
                );
                match run_command_hook(
                    command,
                    &rule.plugin_root,
                    &payload,
                    *timeout,
                    self.use_login_shell_path,
                )
                .await
                {
                    Ok(run) if run.output.status.success() => {
                        let stdout = String::from_utf8_lossy(&run.output.stdout);
                        if let Some(banner) = extract_banner(stdout.trim()) {
                            banners.push(banner);
                        }
                    }
                    Ok(run) => {
                        warn!(
                            plugin = %rule.plugin_name,
                            event = %event,
                            command = %command,
                            "hook exited with {:?}: {}",
                            run.output.status.code(),
                            String::from_utf8_lossy(&run.output.stderr).trim(),
                        );
                    }
                    Err(err) => {
                        warn!(
                            plugin = %rule.plugin_name,
                            event = %event,
                            command = %command,
                            error = %err,
                            "Plugin hook failed",
                        );
                    }
                }
            }
        }

        banners
    }

    /// Like [`Self::emit`], but stops at the first rule that denies the event
    /// and returns the denial. A hook denies by exiting with status code 2
    /// (reason on stderr) or by printing `{"decision":"block","reason":"..."}`
    pub async fn emit_blocking(&self, event: HookEvent, ctx: HookContext) -> HookDecision {
        self.emit_blocking_with_outcome(event, ctx).await.decision
    }

    pub(crate) async fn emit_blocking_with_outcome(
        &self,
        event: HookEvent,
        ctx: HookContext,
    ) -> HookChainOutcome {
        let matched = self.matching_actions(event, &ctx);
        if matched.is_empty() {
            return HookChainOutcome::allow(false);
        }

        let payload = match serde_json::to_string(&ctx) {
            Ok(payload) => payload,
            Err(err) => {
                warn!(event = %event, error = %err, "Failed to serialize hook context");
                return serialization_failure_outcome(&matched, event);
            }
        };

        let mut policy_evaluated = false;
        let mut failed = false;

        for (rule, action) in matched {
            let LoadedAction::Command {
                command,
                timeout,
                on_failure,
            } = action;
            let (verdict, evaluated, already_logged) = match self
                .run_action(event, &ctx.session_id, rule, command, &payload, *timeout)
                .await
            {
                Ok(run) => {
                    let verdict = classify_run(&run);
                    let evaluated = run.output.status.success()
                        || matches!(verdict, HookVerdict::PolicyDeny { .. });
                    (verdict, evaluated, false)
                }
                Err(err) => {
                    warn!(
                        plugin = %rule.plugin_name,
                        event = %event,
                        command = %command,
                        error = %format!("{err:#}"),
                        "Plugin hook could not be executed",
                    );
                    (
                        HookVerdict::HookFailure {
                            reason: COMMAND_FAILED_REASON.to_string(),
                        },
                        false,
                        true,
                    )
                }
            };
            policy_evaluated |= evaluated;

            match apply_verdict(verdict, *on_failure, event) {
                ChainStep::Allowed => {}
                ChainStep::FailedOpen { reason } => {
                    if !already_logged {
                        warn!(
                            plugin = %rule.plugin_name,
                            event = %event,
                            command = %command,
                            reason = %reason,
                            "Plugin hook failed; continuing without it",
                        );
                    }
                    failed = true;
                }
                ChainStep::Denied { reason, cause } => {
                    info!(
                        plugin = %rule.plugin_name,
                        event = %event,
                        command = %command,
                        cause = cause.as_str(),
                        reason = %reason,
                        "Plugin hook denied tool call",
                    );
                    return HookChainOutcome {
                        decision: HookDecision::Deny {
                            reason,
                            plugin: rule.plugin_name.clone(),
                        },
                        policy_evaluated,
                        cause: Some(cause),
                    };
                }
            }
        }

        HookChainOutcome {
            decision: HookDecision::Allow,
            policy_evaluated,
            cause: failed.then_some(HookOutcomeCause::HookFailure),
        }
    }

    fn matching_actions(
        &self,
        event: HookEvent,
        ctx: &HookContext,
    ) -> Vec<(&LoadedRule, &LoadedAction)> {
        let Some(rules) = self.rules.get(&event) else {
            return Vec::new();
        };
        let target = ctx.matcher_context.as_deref().unwrap_or("");
        rules
            .iter()
            .filter(|rule| rule.matcher.as_ref().is_none_or(|m| m.is_match(target)))
            .flat_map(|rule| rule.actions.iter().map(move |action| (rule, action)))
            .collect()
    }
}

fn serialization_failure_outcome(
    matched: &[(&LoadedRule, &LoadedAction)],
    event: HookEvent,
) -> HookChainOutcome {
    let reason = SERIALIZATION_FAILED_REASON.to_string();
    let blocker = matched
        .iter()
        .find(|(_, action)| action.on_failure() == OnFailure::Block);
    match blocker {
        Some((rule, _)) if event == HookEvent::PreToolUse => HookChainOutcome {
            decision: HookDecision::Deny {
                reason,
                plugin: rule.plugin_name.clone(),
            },
            policy_evaluated: false,
            cause: Some(HookOutcomeCause::HookFailure),
        },
        _ => HookChainOutcome {
            decision: HookDecision::Allow,
            policy_evaluated: false,
            cause: Some(HookOutcomeCause::HookFailure),
        },
    }
}

#[derive(Debug)]
struct HookRun {
    output: std::process::Output,
    stdin_delivered: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum HookVerdict {
    Allow,
    PolicyDeny { reason: String },
    HookFailure { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ChainStep {
    Allowed,
    Denied {
        reason: String,
        cause: HookOutcomeCause,
    },
    FailedOpen {
        reason: String,
    },
}

fn apply_verdict(verdict: HookVerdict, on_failure: OnFailure, event: HookEvent) -> ChainStep {
    match verdict {
        HookVerdict::Allow => ChainStep::Allowed,
        HookVerdict::PolicyDeny { reason } => ChainStep::Denied {
            reason,
            cause: HookOutcomeCause::PolicyDenial,
        },
        HookVerdict::HookFailure { reason } => {
            if on_failure == OnFailure::Block && event == HookEvent::PreToolUse {
                ChainStep::Denied {
                    reason,
                    cause: HookOutcomeCause::HookFailure,
                }
            } else {
                ChainStep::FailedOpen { reason }
            }
        }
    }
}

fn extract_banner(stdout: &str) -> Option<String> {
    if !stdout.starts_with('{') {
        return None;
    }

    #[derive(Deserialize)]
    struct BannerResp {
        banner: Option<String>,
    }

    let parsed: BannerResp = serde_json::from_str(stdout).ok()?;
    parsed.banner.filter(|b| !b.is_empty())
}

fn classify_run(run: &HookRun) -> HookVerdict {
    let verdict = classify_output(&run.output);
    if run.stdin_delivered || matches!(verdict, HookVerdict::PolicyDeny { .. }) {
        return verdict;
    }
    HookVerdict::HookFailure {
        reason: STDIN_DELIVERY_FAILED_REASON.to_string(),
    }
}

fn classify_output(output: &std::process::Output) -> HookVerdict {
    const DEFAULT_DENY: &str = "denied by plugin hook";
    let non_empty = |s: String| if s.is_empty() { DEFAULT_DENY.into() } else { s };

    if output.status.code() == Some(2) {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return HookVerdict::PolicyDeny {
            reason: non_empty(stderr),
        };
    }

    #[derive(Deserialize)]
    struct Resp {
        decision: Option<String>,
        reason: Option<String>,
    }

    let Ok(stdout) = std::str::from_utf8(&output.stdout) else {
        return HookVerdict::HookFailure {
            reason: "the hook wrote invalid UTF-8 to stdout".to_string(),
        };
    };
    let trimmed = stdout.trim();
    let parsed = trimmed
        .starts_with('{')
        .then(|| serde_json::from_str::<Resp>(trimmed).ok())
        .flatten();
    let (decision, reason) = match parsed {
        Some(resp) => (resp.decision, resp.reason),
        None => (None, None),
    };

    if decision.as_deref() == Some("block") {
        return HookVerdict::PolicyDeny {
            reason: non_empty(reason.unwrap_or_default()),
        };
    }
    if output.status.code() == Some(0)
        && (trimmed.is_empty() || decision.as_deref() == Some("allow"))
    {
        return HookVerdict::Allow;
    }

    HookVerdict::HookFailure {
        reason: match output.status.code() {
            Some(0) => "the hook exited 0 without an allow or block decision on stdout".to_string(),
            Some(code) => format!("the hook exited with status {code} and no usable decision"),
            None => "the hook was terminated by a signal".to_string(),
        },
    }
}

fn load_hooks_file(
    path: &Path,
    plugin_name: &str,
    plugin_root: &Path,
) -> Result<HashMap<HookEvent, Vec<LoadedRule>>> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let parsed: HooksFile =
        serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;

    let mut out: HashMap<HookEvent, Vec<LoadedRule>> = HashMap::new();
    for (event_name, raw_payload) in parsed.hooks {
        let Some(event) = HookEvent::from_name(&event_name) else {
            debug!(plugin = plugin_name, event = %event_name, "Ignoring unknown hook event");
            continue;
        };
        let raw_rules: Vec<RawHookRule> = serde_json::from_value(raw_payload)
            .with_context(|| format!("reading `{event_name}` rules in {}", path.display()))?;

        for raw in raw_rules {
            let mut selected = Vec::new();
            for raw_action in raw.hooks {
                if select_action(&raw_action, plugin_name, path)? == ActionSelection::Ignored {
                    continue;
                }
                let action: RawCommandAction = serde_json::from_value(raw_action)
                    .with_context(|| format!("reading hook action in {}", path.display()))?;
                selected.push(action);
            }

            let matcher = match raw.matcher.as_deref().filter(|s| !s.is_empty()) {
                Some(pattern) => match Regex::new(pattern) {
                    Ok(re) => Some(re),
                    Err(err) => {
                        warn!(
                            plugin = plugin_name,
                            pattern,
                            error = %err,
                            "Invalid hook matcher regex; skipping rule",
                        );
                        continue;
                    }
                },
                None => None,
            };

            let mut actions = Vec::new();
            for action in selected {
                let timeout =
                    Duration::from_secs(action.timeout.unwrap_or(DEFAULT_HOOK_TIMEOUT_SECS));
                let on_failure = if event == HookEvent::PreToolUse {
                    match action.on_failure {
                        None => OnFailure::Allow,
                        Some(value) => serde_json::from_value::<OnFailure>(value)
                            .with_context(|| format!("reading on_failure in {}", path.display()))?,
                    }
                } else {
                    OnFailure::Allow
                };
                actions.push(LoadedAction::Command {
                    command: action.command,
                    timeout,
                    on_failure,
                });
            }

            if actions.is_empty() {
                continue;
            }

            out.entry(event).or_default().push(LoadedRule {
                plugin_name: plugin_name.to_string(),
                plugin_root: plugin_root.to_path_buf(),
                matcher,
                actions,
            });
        }
    }

    Ok(out)
}

async fn run_command_hook(
    raw_command: &str,
    plugin_root: &Path,
    payload: &str,
    timeout: Duration,
    use_login_shell_path: bool,
) -> Result<HookRun> {
    match tokio::time::timeout(
        timeout,
        run_command_hook_inner(raw_command, plugin_root, payload, use_login_shell_path),
    )
    .await
    {
        Ok(res) => res,
        Err(_) => anyhow::bail!("hook `{raw_command}` timed out after {:?}", timeout),
    }
}

async fn run_command_hook_inner(
    raw_command: &str,
    plugin_root: &Path,
    payload: &str,
    use_login_shell_path: bool,
) -> Result<HookRun> {
    let command = expand_plugin_root(raw_command, plugin_root);
    let path = if use_login_shell_path {
        hook_path().await
    } else {
        None
    };
    let mut process = hook_command(&command, plugin_root, path.as_deref());
    process
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = process
        .spawn()
        .with_context(|| format!("spawning hook `{command}`"))?;

    let mut stdin_delivered = true;
    if let Some(mut stdin) = child.stdin.take() {
        if let Err(err) = stdin.write_all(payload.as_bytes()).await {
            stdin_delivered = false;
            warn!(command = %command, error = %err, "Could not deliver the hook payload");
        } else if let Err(err) = stdin.shutdown().await {
            stdin_delivered = false;
            warn!(command = %command, error = %err, "Could not close the hook stdin pipe");
        }
    }

    let output = child
        .wait_with_output()
        .await
        .with_context(|| format!("waiting on hook `{command}`"))?;
    Ok(HookRun {
        output,
        stdin_delivered,
    })
}

fn hook_command(command: &str, plugin_root: &Path, path: Option<&str>) -> Command {
    #[cfg(not(windows))]
    {
        if crate::agents::platform_extensions::developer::shell::is_flatpak() {
            let mut process =
                crate::agents::platform_extensions::developer::shell::flatpak_spawn_command();
            process.arg(format!("--env=PLUGIN_ROOT={}", plugin_root.display()));
            if let Some(path) = path {
                process.arg(format!("--env=PATH={path}"));
            }
            process.arg("sh").arg("-c").arg(command);
            return process;
        }
    }

    let mut process = Command::new("sh");
    process
        .arg("-c")
        .arg(command)
        .env("PLUGIN_ROOT", plugin_root);
    if let Some(path) = path {
        process.env("PATH", path);
    }
    process
}

async fn hook_path() -> Option<String> {
    static HOOK_PATH: OnceLock<tokio::sync::watch::Receiver<Option<String>>> = OnceLock::new();
    let mut rx = HOOK_PATH
        .get_or_init(|| {
            let (tx, rx) = tokio::sync::watch::channel(None);
            tokio::spawn(async move {
                let path = resolve_hook_path().await;
                let _ = tx.send(path);
            });
            rx
        })
        .clone();

    if rx.borrow().is_some() {
        return rx.borrow().clone();
    }
    if rx.changed().await.is_ok() {
        rx.borrow().clone()
    } else {
        None
    }
}

async fn resolve_hook_path() -> Option<String> {
    #[cfg(not(windows))]
    {
        tokio::task::spawn_blocking(|| {
            crate::agents::platform_extensions::developer::shell::resolve_login_shell_path()
                .map(|login| merge_paths(&login, &std::env::var("PATH").unwrap_or_default()))
        })
        .await
        .ok()
        .flatten()
    }
    #[cfg(windows)]
    {
        None
    }
}

fn merge_paths(first: &str, second: &str) -> String {
    let mut seen = std::collections::HashSet::new();
    let mut merged = Vec::new();
    for entry in first.split(':').chain(second.split(':')) {
        if !entry.is_empty() && seen.insert(entry) {
            merged.push(entry);
        }
    }
    merged.join(":")
}

fn expand_plugin_root(command: &str, plugin_root: &Path) -> String {
    command.replace("${PLUGIN_ROOT}", &plugin_root.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::discovery::{DiscoveredPlugin, PluginScope};

    fn write_plugin(root: &Path, name: &str, hooks_json: &str) -> PathBuf {
        let plugin = root.join(name);
        std::fs::create_dir_all(plugin.join("hooks")).unwrap();
        std::fs::write(plugin.join("hooks").join("hooks.json"), hooks_json).unwrap();
        plugin
    }

    fn make_manager(plugins: Vec<DiscoveredPlugin>) -> HookManager {
        HookManager::from_plugins(plugins, false)
    }

    fn action(command: &str) -> Value {
        serde_json::json!({ "type": "command", "command": command })
    }

    fn blocking_action(command: &str) -> Value {
        serde_json::json!({ "type": "command", "command": command, "on_failure": "block" })
    }

    fn manager_for(root: &Path, event: HookEvent, plugins: &[(&str, Vec<Value>)]) -> HookManager {
        let discovered = plugins
            .iter()
            .map(|(name, actions)| {
                let hooks = serde_json::json!({
                    "hooks": { event.name(): [{ "hooks": actions }] }
                })
                .to_string();
                DiscoveredPlugin {
                    name: (*name).to_string(),
                    root: write_plugin(root, name, &hooks),
                    scope: PluginScope::User,
                }
            })
            .collect();
        make_manager(discovered)
    }

    fn blocking_context(event: HookEvent) -> HookContext {
        HookContext::new(event, "s").with_tool("developer__shell", None)
    }

    async fn run_chain(event: HookEvent, actions: Vec<Value>) -> HookChainOutcome {
        let tmp = tempfile::tempdir().unwrap();
        manager_for(tmp.path(), event, &[("p", actions)])
            .emit_blocking_with_outcome(event, blocking_context(event))
            .await
    }

    fn oversized_context(event: HookEvent) -> HookContext {
        let filler = "x".repeat(1024 * 1024);
        HookContext::new(event, "s").with_tool(
            "developer__shell",
            Some(serde_json::json!({ "arg": filler })),
        )
    }

    async fn run_chain_with(
        event: HookEvent,
        actions: Vec<Value>,
        ctx: HookContext,
    ) -> HookChainOutcome {
        let tmp = tempfile::tempdir().unwrap();
        manager_for(tmp.path(), event, &[("p", actions)])
            .emit_blocking_with_outcome(event, ctx)
            .await
    }

    #[cfg(unix)]
    fn exited(code: i32, stdout: &str, stderr: &str) -> std::process::Output {
        use std::os::unix::process::ExitStatusExt;
        std::process::Output {
            status: std::process::ExitStatus::from_raw(code << 8),
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    #[cfg(unix)]
    fn exited_raw(code: i32, stdout: &[u8], stderr: &str) -> std::process::Output {
        use std::os::unix::process::ExitStatusExt;
        std::process::Output {
            status: std::process::ExitStatus::from_raw(code << 8),
            stdout: stdout.to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    #[cfg(unix)]
    fn signalled(signal: i32) -> std::process::Output {
        use std::os::unix::process::ExitStatusExt;
        std::process::Output {
            status: std::process::ExitStatus::from_raw(signal),
            stdout: Vec::new(),
            stderr: Vec::new(),
        }
    }

    #[cfg(unix)]
    #[test]
    fn classify_output_pins_every_boundary() {
        let denies = |reason: &str| HookVerdict::PolicyDeny {
            reason: reason.to_string(),
        };

        assert_eq!(
            classify_output(&exited_raw(2, b"\xff", "  path is protected  ")),
            denies("path is protected")
        );
        assert_eq!(
            classify_output(&exited(2, "", "")),
            denies("denied by plugin hook")
        );
        assert_eq!(
            classify_output(&exited(1, r#"{"decision":"block","reason":"nope"}"#, "")),
            denies("nope")
        );
        assert_eq!(
            classify_output(&exited(0, r#"{"decision":"block"}"#, "")),
            denies("denied by plugin hook")
        );

        for stdout in ["", r#"{"decision":"allow"}"#] {
            assert_eq!(classify_output(&exited(0, stdout, "")), HookVerdict::Allow);
        }

        for output in [
            exited(1, r#"{"decision":"allow"}"#, ""),
            exited(0, r#"{"decision":"maybe"}"#, ""),
            exited(0, r#"{"decision" "allow"}"#, ""),
            exited_raw(0, b"{\"decision\":\"allow\",\"reason\":\"\xff\"}", ""),
            exited(1, "", ""),
            signalled(9),
        ] {
            assert!(
                matches!(classify_output(&output), HookVerdict::HookFailure { .. }),
                "expected a hook failure, got {:?}",
                classify_output(&output)
            );
        }
    }

    #[test]
    fn on_failure_block_denies_pre_tool_use_failures_and_leaves_stop_alone() {
        let failure = || HookVerdict::HookFailure {
            reason: "failed".to_string(),
        };
        let failed_open = ChainStep::FailedOpen {
            reason: "failed".to_string(),
        };
        assert_eq!(
            apply_verdict(failure(), OnFailure::Allow, HookEvent::PreToolUse),
            failed_open
        );
        assert_eq!(
            apply_verdict(failure(), OnFailure::Block, HookEvent::PreToolUse),
            ChainStep::Denied {
                reason: "failed".to_string(),
                cause: HookOutcomeCause::HookFailure,
            }
        );
        assert_eq!(
            apply_verdict(failure(), OnFailure::Block, HookEvent::Stop),
            failed_open
        );

        for event in [HookEvent::PreToolUse, HookEvent::Stop] {
            for mode in [OnFailure::Allow, OnFailure::Block] {
                assert_eq!(
                    apply_verdict(HookVerdict::Allow, mode, event),
                    ChainStep::Allowed
                );
                assert_eq!(
                    apply_verdict(
                        HookVerdict::PolicyDeny {
                            reason: "nope".to_string()
                        },
                        mode,
                        event
                    ),
                    ChainStep::Denied {
                        reason: "nope".to_string(),
                        cause: HookOutcomeCause::PolicyDenial,
                    }
                );
            }
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_blocked_execution_failure_never_leaks_the_command_or_path() {
        const SECRET: &str = "s3cr3t-token";
        let spec = serde_json::json!({
            "type": "command",
            "command": format!("curl -H 'Authorization: Bearer {SECRET}' \u{0}"),
            "on_failure": "block",
        });
        let outcome = run_chain(HookEvent::PreToolUse, vec![spec]).await;
        let HookDecision::Deny { reason, .. } = &outcome.decision else {
            panic!("expected a denial, got {:?}", outcome.decision);
        };
        assert_eq!(reason, COMMAND_FAILED_REASON);
        for leaked in [SECRET, "curl", "nul byte"] {
            assert!(!reason.contains(leaked));
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn subprocess_failures_fail_open_by_default_and_block_when_configured() {
        let cases = [
            (
                "spawn failure",
                serde_json::json!({"type":"command","command":"echo \u{0}"}),
                false,
            ),
            (
                "timeout",
                serde_json::json!({"type":"command","command":"sleep 30","timeout":0}),
                false,
            ),
            ("termination by signal", action("kill -9 $$"), false),
            ("an unexpected exit", action("exit 3"), false),
            (
                "malformed output",
                action(r#"printf '%s' '{"decision" "allow"}'"#),
                true,
            ),
            ("invalid UTF-8", action(r#"printf '\377'"#), true),
            (
                "allow JSON with a non-zero exit",
                action(r#"printf '%s' '{"decision":"allow"}'; exit 1"#),
                false,
            ),
        ];

        for (label, mut spec, evaluated) in cases {
            let allowed = run_chain(HookEvent::PreToolUse, vec![spec.clone()]).await;
            assert_eq!(
                allowed.decision,
                HookDecision::Allow,
                "{label} must fail open by default"
            );
            assert_eq!(
                allowed.cause,
                Some(HookOutcomeCause::HookFailure),
                "{label}"
            );
            assert_eq!(
                allowed.policy_evaluated, evaluated,
                "{label}: policy_evaluated is exit 0 or an explicit decision, \
                 not whether the hook produced a usable one"
            );

            spec["on_failure"] = serde_json::json!("block");
            let blocked = run_chain(HookEvent::PreToolUse, vec![spec.clone()]).await;
            assert!(
                matches!(blocked.decision, HookDecision::Deny { .. }),
                "{label} must deny under on_failure block, got {:?}",
                blocked.decision
            );
            assert_eq!(
                blocked.cause,
                Some(HookOutcomeCause::HookFailure),
                "{label}"
            );
            assert_eq!(
                blocked.policy_evaluated, evaluated,
                "{label}: on_failure must not change policy_evaluated"
            );
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_hook_that_never_received_the_payload_cannot_allow_but_can_still_deny() {
        let event = HookEvent::PreToolUse;

        let open = run_chain_with(event, vec![action("exit 0")], oversized_context(event)).await;
        assert_eq!(
            open.decision,
            HookDecision::Allow,
            "the default still allows"
        );
        assert_eq!(
            open.cause,
            Some(HookOutcomeCause::HookFailure),
            "a hook that never saw the request produced no decision",
        );
        assert!(open.policy_evaluated, "it did exit 0");

        let printed_allow = run_chain_with(
            event,
            vec![blocking_action(r#"printf '%s' '{"decision":"allow"}'"#)],
            oversized_context(event),
        )
        .await;
        let HookDecision::Deny { reason, .. } = &printed_allow.decision else {
            panic!(
                "an allow from a hook that never saw the request must not stand under block, got {:?}",
                printed_allow.decision
            );
        };
        assert_eq!(reason, STDIN_DELIVERY_FAILED_REASON);
        assert_eq!(printed_allow.cause, Some(HookOutcomeCause::HookFailure));
        assert!(printed_allow.policy_evaluated, "it did exit 0");

        let denied = run_chain_with(
            event,
            vec![action("echo refused by policy >&2; exit 2")],
            oversized_context(event),
        )
        .await;
        assert!(matches!(denied.decision, HookDecision::Deny { .. }));
        assert_eq!(denied.cause, Some(HookOutcomeCause::PolicyDenial));
    }

    #[tokio::test]
    async fn decisions_are_unchanged_by_on_failure_block() {
        let block = r#"printf '%s' '{"decision":"block","reason":"nope"}'"#;
        let denied = HookDecision::Deny {
            reason: "nope".to_string(),
            plugin: "p".to_string(),
        };

        for spec in [action(block), blocking_action(block)] {
            let outcome = run_chain(HookEvent::PreToolUse, vec![spec]).await;
            assert_eq!(outcome.decision, denied);
            assert_eq!(outcome.cause, Some(HookOutcomeCause::PolicyDenial));
            assert!(outcome.policy_evaluated);
        }

        for spec in [
            action("cat >/dev/null 2>&1; exit 0"),
            blocking_action(r#"cat >/dev/null 2>&1; printf '%s' '{"decision":"allow"}'"#),
        ] {
            let outcome = run_chain(HookEvent::PreToolUse, vec![spec]).await;
            assert_eq!(outcome.decision, HookDecision::Allow);
            assert_eq!(outcome.cause, None);
            assert!(outcome.policy_evaluated);
        }
    }

    #[tokio::test]
    async fn mixed_chains_preserve_evaluation_and_report_the_final_cause() {
        let failure = run_chain(
            HookEvent::PreToolUse,
            vec![action("exit 0"), blocking_action("exit 3")],
        )
        .await;

        assert!(matches!(failure.decision, HookDecision::Deny { .. }));
        assert_eq!(failure.cause, Some(HookOutcomeCause::HookFailure));
        assert!(failure.policy_evaluated);

        let denial = run_chain(
            HookEvent::PreToolUse,
            vec![
                action("exit 3"),
                action(r#"printf '%s' '{"decision":"block","reason":"nope"}'"#),
            ],
        )
        .await;

        assert_eq!(
            denial.decision,
            HookDecision::Deny {
                reason: "nope".to_string(),
                plugin: "p".to_string(),
            }
        );
        assert_eq!(denial.cause, Some(HookOutcomeCause::PolicyDenial));
        assert!(denial.policy_evaluated);
    }

    #[tokio::test]
    async fn a_block_mode_failure_stops_the_chain() {
        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join("later-ran.txt");
        let mgr = manager_for(
            tmp.path(),
            HookEvent::PreToolUse,
            &[(
                "p",
                vec![
                    blocking_action("exit 3"),
                    action(&format!("touch {}", marker.to_string_lossy())),
                ],
            )],
        );

        let outcome = mgr
            .emit_blocking_with_outcome(
                HookEvent::PreToolUse,
                blocking_context(HookEvent::PreToolUse),
            )
            .await;

        assert!(matches!(outcome.decision, HookDecision::Deny { .. }));
        assert!(!marker.exists(), "the chain must stop at the denial");
    }

    #[tokio::test]
    async fn no_matching_rule_allows_without_evaluating_a_policy() {
        let tmp = tempfile::tempdir().unwrap();
        let hooks = serde_json::json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "developer__shell",
                    "hooks": [blocking_action("exit 3")],
                }]
            }
        })
        .to_string();
        let root = write_plugin(tmp.path(), "p", &hooks);
        let mgr = make_manager(vec![DiscoveredPlugin {
            name: "p".into(),
            root,
            scope: PluginScope::User,
        }]);

        let outcome = mgr
            .emit_blocking_with_outcome(
                HookEvent::PreToolUse,
                HookContext::new(HookEvent::PreToolUse, "s").with_tool("other__tool", None),
            )
            .await;

        assert_eq!(outcome.decision, HookDecision::Allow);
        assert!(!outcome.policy_evaluated);
        assert_eq!(outcome.cause, None);
    }

    #[test]
    fn serialization_failure_fails_open_by_default_and_blocks_when_configured() {
        let outcome = |event: HookEvent, plugins: &[(&str, Vec<Value>)]| {
            let tmp = tempfile::tempdir().unwrap();
            let manager = manager_for(tmp.path(), event, plugins);
            let ctx = blocking_context(event);
            let matched = manager.matching_actions(event, &ctx);
            assert!(!matched.is_empty(), "the fixture must match an action");
            serialization_failure_outcome(&matched, event)
        };

        let open = outcome(
            HookEvent::PreToolUse,
            &[("fail-open", vec![action("exit 0")])],
        );
        assert_eq!(open.decision, HookDecision::Allow);
        assert_eq!(open.cause, Some(HookOutcomeCause::HookFailure));
        assert!(!open.policy_evaluated);

        let closed = outcome(
            HookEvent::PreToolUse,
            &[
                ("fail-open", vec![action("exit 0")]),
                ("fail-closed", vec![blocking_action("exit 0")]),
            ],
        );
        let HookDecision::Deny { plugin, reason } = &closed.decision else {
            panic!("expected a denial, got {:?}", closed.decision);
        };
        assert_eq!(plugin, "fail-closed", "the first blocking action owns it");
        assert_eq!(reason, SERIALIZATION_FAILED_REASON);
        assert_eq!(closed.cause, Some(HookOutcomeCause::HookFailure));
        assert!(!closed.policy_evaluated);

        let stop = outcome(
            HookEvent::Stop,
            &[("fail-closed", vec![blocking_action("exit 0")])],
        );
        assert_eq!(stop.decision, HookDecision::Allow);
        assert!(!stop.policy_evaluated);
    }

    fn loaded_on_failure(hooks_json: &str) -> Result<OnFailure> {
        let tmp = tempfile::tempdir().unwrap();
        let root = write_plugin(tmp.path(), "p", hooks_json);
        let loaded = load_hooks_file(&root.join("hooks").join("hooks.json"), "p", &root)?;
        Ok(loaded[&HookEvent::PreToolUse][0].actions[0].on_failure())
    }

    #[test]
    fn on_failure_values_are_validated() {
        let rule = |extra: &str| {
            format!(
                r#"{{"hooks":{{"PreToolUse":[{{"hooks":[{{"type":"command","command":"echo"{extra}}}]}}]}}}}"#
            )
        };

        for (extra, expected) in [
            ("", Some(OnFailure::Allow)),
            (r#","on_failure":"allow""#, Some(OnFailure::Allow)),
            (r#","on_failure":"block""#, Some(OnFailure::Block)),
            (r#","on_failure":"deny""#, None),
            (r#","on_failure":null"#, None),
        ] {
            match expected {
                Some(expected) => assert_eq!(loaded_on_failure(&rule(extra)).unwrap(), expected),
                None => assert!(loaded_on_failure(&rule(extra)).is_err()),
            }
        }
    }

    #[test]
    fn on_failure_outside_pre_tool_use_is_ignored_not_validated() {
        for ignored in [r#""retry""#, "null"] {
            let hooks = format!(
                r#"{{"hooks":{{
                    "Stop":[{{"hooks":[{{"type":"command","command":"echo","on_failure":{ignored}}}]}}],
                    "PreToolUse":[{{"hooks":[{{"type":"command","command":"echo refused by policy >&2; exit 2","on_failure":"block"}}]}}]
                }}}}"#
            );
            let tmp = tempfile::tempdir().unwrap();
            let root = write_plugin(tmp.path(), "p", &hooks);

            let loaded = load_hooks_file(&root.join("hooks").join("hooks.json"), "p", &root)
                .unwrap_or_else(|err| {
                    panic!("on_failure {ignored} on Stop must not reject the file: {err:#}")
                });

            let stop = &loaded[&HookEvent::Stop][0].actions;
            assert_eq!(stop.len(), 1, "on_failure {ignored}");
            assert_eq!(
                stop[0].on_failure(),
                OnFailure::Allow,
                "a Stop action loads with the default: {ignored}"
            );

            let pre = &loaded[&HookEvent::PreToolUse][0].actions;
            assert_eq!(
                pre[0].on_failure(),
                OnFailure::Block,
                "the PreToolUse action keeps its own on_failure: {ignored}"
            );
        }
    }

    fn load_hooks_json(hooks_json: &str) -> Result<HashMap<HookEvent, Vec<LoadedRule>>> {
        let tmp = tempfile::tempdir().unwrap();
        let root = write_plugin(tmp.path(), "p", hooks_json);
        load_hooks_file(&root.join("hooks").join("hooks.json"), "p", &root)
    }

    fn surviving_pre_tool_use_command(hooks_json: &str) -> String {
        let tmp = tempfile::tempdir().unwrap();
        let root = write_plugin(tmp.path(), "p", hooks_json);
        let loaded = load_hooks_file(&root.join("hooks").join("hooks.json"), "p", &root)
            .unwrap_or_else(|err| panic!("the file must load: {err:#}"));
        let rules = &loaded[&HookEvent::PreToolUse];
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].actions.len(), 1);
        let LoadedAction::Command { command, .. } = &rules[0].actions[0];
        command.clone()
    }

    const DENYING_COMMAND: &str =
        r#"{"type":"command","command":"echo refused by policy >&2; exit 2"}"#;

    #[test]
    fn an_unknown_event_payload_is_opaque_whatever_its_shape() {
        let hooks = format!(
            r#"{{"hooks":{{
                "SomeFutureEvent":{{"mode":"batch","rules":42,"on_failure":"retry"}},
                "AnotherFutureEvent":null,
                "ThirdFutureEvent":[{{"matcher":{{"kind":"glob"}},"hooks":"soon","on_failure":null}}],
                "PreToolUse":[{{"hooks":[{DENYING_COMMAND}]}}]
            }}}}"#
        );
        assert_eq!(
            surviving_pre_tool_use_command(&hooks),
            "echo refused by policy >&2; exit 2"
        );
    }

    #[test]
    fn action_selection_preserves_ignored_and_null_shapes() {
        let hooks = r#"{"hooks":{"PreToolUse":[{"hooks":[
                {"type":"webhook","command":{"url":"https://example.invalid"},"timeout":"soon","on_failure":{"mode":"retry"},"headers":[1,2]},
                {"type":"command","command":null},
                {"type":"command","timeout":"5s","on_failure":{"mode":"retry"}},
                {"type":null,"command":"echo refused by policy >&2; exit 2"}
            ]}]}}"#;
        assert_eq!(
            surviving_pre_tool_use_command(hooks),
            "echo refused by policy >&2; exit 2"
        );
    }

    #[test]
    fn malformed_selected_configuration_is_rejected() {
        for (hooks, expected) in [
            (
                r#"{"hooks":{"PreToolUse":[{"matcher":{"kind":"glob"},"hooks":[{"type":"command","command":"echo"}]}]}}"#,
                "expected a string",
            ),
            (
                r#"{"hooks":{"PreToolUse":[{"hooks":[{"type":42,"command":"echo"}]}]}}"#,
                "must be a string",
            ),
            (r#"{"hooks":{"PreToolUse":{}}}"#, "expected a sequence"),
            (
                r#"{"hooks":{"PreToolUse":[{"hooks":["echo"]}]}}"#,
                "must be an object",
            ),
            (
                r#"{"hooks":{"PreToolUse":[{"hooks":[{"type":"command","command":"echo","timeout":"5s"}]}]}}"#,
                "expected u64",
            ),
            (
                r#"{"hooks":{"PreToolUse":[{"hooks":[{"type":"command","command":["echo"]}]}]}}"#,
                "must be a string",
            ),
        ] {
            let err = load_hooks_json(hooks).unwrap_err();
            assert!(
                format!("{err:#}").contains(expected),
                "expected {expected:?}, got {err:#}"
            );
        }
    }

    #[test]
    fn an_invalid_matcher_skips_its_rule_before_on_failure_is_read() {
        let hooks = format!(
            r#"{{"hooks":{{"PreToolUse":[
                {{"matcher":"[unclosed","hooks":[{{"type":"command","command":"echo","on_failure":"retry"}}]}},
                {{"hooks":[{DENYING_COMMAND}]}}
            ]}}}}"#
        );
        assert_eq!(
            surviving_pre_tool_use_command(&hooks),
            "echo refused by policy >&2; exit 2"
        );
    }

    #[test]
    fn denial_wording_separates_policy_from_failure() {
        let deny = |cause: HookOutcomeCause, reason: &str| {
            HookChainOutcome {
                decision: HookDecision::Deny {
                    reason: reason.to_string(),
                    plugin: "guard".to_string(),
                },
                policy_evaluated: cause == HookOutcomeCause::PolicyDenial,
                cause: Some(cause),
            }
            .denial()
            .expect("a denial reports a refusal")
        };

        let policy = deny(HookOutcomeCause::PolicyDenial, "path is protected");
        assert_eq!(policy.error_type, "hook_denied");
        assert!(policy
            .message
            .contains("denied by policy hook `guard`: path is protected"));
        assert!(policy.message.contains("Do not retry"));

        let failure = deny(
            HookOutcomeCause::HookFailure,
            "the hook exited with status 3 and no usable decision",
        );
        assert_eq!(failure.error_type, "hook_failed");
        assert!(failure.message.contains(
            "blocked because policy hook `guard` could not complete: \
             the hook exited with status 3 and no usable decision"
        ));
        assert!(
            !failure.message.contains("Do not retry")
                && !failure.message.contains("denied by policy hook"),
            "a failure must not be worded as a policy decision: {}",
            failure.message
        );

        assert!(HookChainOutcome::allow(true).denial().is_none());
    }

    /// `decision` is "allow" or "deny" and nothing else, and `blocked_by` and
    /// `reason` appear only on the deny arm — absent from an allow payload
    /// rather than present as null or an empty string.
    #[test]
    fn pre_tool_use_result_payload_reports_decision_and_denies_alone_carry_the_plugin() {
        let payload = |outcome: &HookChainOutcome| -> Value {
            let ctx = HookContext::new(HookEvent::PreToolUseResult, "session-1")
                .with_tool("developer__shell", None)
                .with_tool_call_id("call-1")
                .with_pre_tool_use_outcome(outcome);
            serde_json::from_str(&serde_json::to_string(&ctx).unwrap()).unwrap()
        };

        let allow = payload(&HookChainOutcome::allow(true));
        assert_eq!(allow["decision"], "allow");
        assert_eq!(allow["policy_evaluated"], true);
        assert_eq!(allow["tool_call_id"], "call-1");
        assert!(
            allow.get("blocked_by").is_none(),
            "allow payload must omit blocked_by entirely, got {:?}",
            allow.get("blocked_by")
        );
        assert!(
            allow.get("reason").is_none(),
            "allow payload must omit reason entirely, got {:?}",
            allow.get("reason")
        );

        assert!(
            allow.get("cause").is_none(),
            "a clean allow payload must omit cause entirely, got {:?}",
            allow.get("cause")
        );

        let deny = payload(&HookChainOutcome {
            decision: HookDecision::Deny {
                reason: "blocked by test policy".to_string(),
                plugin: "test-plugin".to_string(),
            },
            policy_evaluated: true,
            cause: Some(HookOutcomeCause::PolicyDenial),
        });
        assert_eq!(deny["decision"], "deny");
        assert_eq!(deny["blocked_by"], "test-plugin");
        assert_eq!(deny["reason"], "blocked by test policy");
        assert_eq!(deny["cause"], "policy_denial");

        let failed_open = payload(&HookChainOutcome {
            decision: HookDecision::Allow,
            policy_evaluated: false,
            cause: Some(HookOutcomeCause::HookFailure),
        });
        assert_eq!(failed_open["decision"], "allow");
        assert_eq!(failed_open["cause"], "hook_failure");

        let failed_closed = payload(&HookChainOutcome {
            decision: HookDecision::Deny {
                reason: "the hook exited with status 3 and no usable decision".to_string(),
                plugin: "test-plugin".to_string(),
            },
            policy_evaluated: false,
            cause: Some(HookOutcomeCause::HookFailure),
        });
        assert_eq!(failed_closed["decision"], "deny");
        assert_eq!(failed_closed["cause"], "hook_failure");

        for value in [&allow, &deny] {
            let decision = value["decision"].as_str().unwrap();
            assert!(
                matches!(decision, "allow" | "deny"),
                "decision must be allow or deny, got {decision}"
            );
        }

        let unevaluated = payload(&HookChainOutcome::allow(false));
        assert_eq!(unevaluated["decision"], "allow");
        assert_eq!(unevaluated["policy_evaluated"], false);
    }

    /// A hook is an evaluation only if it exited 0 or returned a decision. A
    /// non-zero exit carrying no decision means the hook never answered, and an
    /// earlier hook that did answer keeps the aggregate true.
    #[tokio::test]
    async fn policy_evaluated_counts_clean_exits_and_decisions_only() {
        let plugin = |root: &Path, name: &str, command: &str| -> DiscoveredPlugin {
            let hooks = format!(
                r#"{{"hooks":{{"PreToolUse":[{{"hooks":[{{"type":"command","command":"{command}"}}]}}]}}}}"#
            );
            DiscoveredPlugin {
                name: name.into(),
                root: write_plugin(root, name, &hooks),
                scope: PluginScope::User,
            }
        };
        let ctx =
            || HookContext::new(HookEvent::PreToolUse, "s").with_tool("developer__shell", None);

        // Case 1: the only hook exits non-zero with nothing on stdout. It gave no
        // decision, so the call is allowed and nothing was evaluated.
        let tmp = tempfile::tempdir().unwrap();
        let mgr = make_manager(vec![plugin(tmp.path(), "abnormal", "exit 3")]);
        let outcome = mgr
            .emit_blocking_with_outcome(HookEvent::PreToolUse, ctx())
            .await;
        assert_eq!(outcome.decision, HookDecision::Allow);
        assert!(
            !outcome.policy_evaluated,
            "a sole hook exiting 3 with no decision must not count as evaluated",
        );

        // Case 2: one hook exits 0 and another exits non-zero. policy_evaluated is
        // an at-least-one aggregate, so the clean exit keeps it true.
        let tmp = tempfile::tempdir().unwrap();
        let mgr = make_manager(vec![
            plugin(tmp.path(), "a", "exit 0"),
            plugin(tmp.path(), "b", "exit 3"),
        ]);
        let outcome = mgr
            .emit_blocking_with_outcome(HookEvent::PreToolUse, ctx())
            .await;
        assert_eq!(outcome.decision, HookDecision::Allow);
        assert!(
            outcome.policy_evaluated,
            "a hook that exited 0 must keep policy_evaluated true when a later hook fails",
        );

        // Case 3: the only hook exits 2 with a reason. That is a decision, so it
        // both denies and counts as evaluated.
        let tmp = tempfile::tempdir().unwrap();
        let mgr = make_manager(vec![plugin(
            tmp.path(),
            "denier",
            "echo refused by policy >&2; exit 2",
        )]);
        let outcome = mgr
            .emit_blocking_with_outcome(HookEvent::PreToolUse, ctx())
            .await;
        assert_eq!(
            outcome.decision,
            HookDecision::Deny {
                reason: "refused by policy".to_string(),
                plugin: "denier".to_string(),
            }
        );
        assert!(
            outcome.policy_evaluated,
            "an exit 2 decision must count as evaluated",
        );
    }

    /// PreToolUseResult honours its matcher against the tool name like every
    /// other tool-scoped event, so a subscriber can watch one tool rather than
    /// every call.
    #[tokio::test]
    async fn pre_tool_use_result_matcher_targets_the_tool_name() {
        let tmp = tempfile::tempdir().unwrap();
        let root = write_plugin(
            tmp.path(),
            "p",
            r#"{"hooks":{"PreToolUseResult":[{"matcher":"^developer__shell$","hooks":[{"type":"command","command":"echo ran >> \"$PLUGIN_ROOT/marker.log\""}]}]}}"#,
        );
        let marker = root.join("marker.log");
        let mgr = make_manager(vec![DiscoveredPlugin {
            name: "p".into(),
            root,
            scope: PluginScope::User,
        }]);
        let lines = || {
            std::fs::read_to_string(&marker)
                .unwrap_or_default()
                .lines()
                .filter(|line| !line.trim().is_empty())
                .count()
        };

        mgr.emit(
            HookEvent::PreToolUseResult,
            HookContext::new(HookEvent::PreToolUseResult, "s").with_tool("developer__shell", None),
        )
        .await;
        assert_eq!(lines(), 1, "the matching tool must run the hook");

        mgr.emit(
            HookEvent::PreToolUseResult,
            HookContext::new(HookEvent::PreToolUseResult, "s").with_tool("other__tool", None),
        )
        .await;
        assert_eq!(lines(), 1, "a non-matching tool must not run the hook");
    }

    #[test]
    fn ignores_unknown_events() {
        let tmp = tempfile::tempdir().unwrap();
        let root = write_plugin(
            tmp.path(),
            "p",
            r#"{"hooks":{"NotARealEvent":[{"hooks":[{"type":"command","command":"echo"}]}]}}"#,
        );
        let mgr = make_manager(vec![DiscoveredPlugin {
            name: "p".into(),
            root,
            scope: PluginScope::User,
        }]);
        assert!(!mgr.has_hooks(HookEvent::PreToolUse));
    }

    #[test]
    fn loads_matcher_and_command() {
        let tmp = tempfile::tempdir().unwrap();
        let root = write_plugin(
            tmp.path(),
            "p",
            r#"{"hooks":{"PostToolUse":[{"matcher":"developer__.*","hooks":[{"type":"command","command":"echo hi"}]}]}}"#,
        );
        let mgr = make_manager(vec![DiscoveredPlugin {
            name: "p".into(),
            root,
            scope: PluginScope::User,
        }]);
        assert!(mgr.has_hooks(HookEvent::PostToolUse));
    }

    #[test]
    fn invalid_matcher_skipped_without_panic() {
        let tmp = tempfile::tempdir().unwrap();
        let root = write_plugin(
            tmp.path(),
            "p",
            r#"{"hooks":{"PostToolUse":[{"matcher":"[invalid","hooks":[{"type":"command","command":"echo"}]}]}}"#,
        );
        let mgr = make_manager(vec![DiscoveredPlugin {
            name: "p".into(),
            root,
            scope: PluginScope::User,
        }]);
        assert!(!mgr.has_hooks(HookEvent::PostToolUse));
    }

    #[tokio::test]
    async fn emit_runs_command_with_plugin_root_substitution() {
        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join("ran.txt");
        let marker_path = marker.to_string_lossy().into_owned();
        let hooks = format!(
            r#"{{"hooks":{{"SessionStart":[{{"hooks":[{{"type":"command","command":"sh -c 'echo $PLUGIN_ROOT > {marker}'"}}]}}]}}}}"#,
            marker = marker_path,
        );
        let root = write_plugin(tmp.path(), "p", &hooks);
        let mgr = make_manager(vec![DiscoveredPlugin {
            name: "p".into(),
            root: root.clone(),
            scope: PluginScope::User,
        }]);

        mgr.emit(
            HookEvent::SessionStart,
            HookContext::new(HookEvent::SessionStart, "session-1"),
        )
        .await;

        let written = std::fs::read_to_string(&marker).unwrap();
        assert_eq!(written.trim(), root.to_string_lossy());
    }

    #[tokio::test]
    async fn stop_hook_emit_blocking_returns_denial() {
        let tmp = tempfile::tempdir().unwrap();
        let root = write_plugin(
            tmp.path(),
            "p",
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"printf '%s' '{\"decision\":\"block\",\"reason\":\"say something first\"}'"}]}]}}"#,
        );
        let mgr = make_manager(vec![DiscoveredPlugin {
            name: "p".into(),
            root,
            scope: PluginScope::User,
        }]);

        let decision = mgr
            .emit_blocking(HookEvent::Stop, HookContext::new(HookEvent::Stop, "s"))
            .await;

        assert_eq!(
            decision,
            HookDecision::Deny {
                reason: "say something first".into(),
                plugin: "p".into(),
            }
        );
    }

    #[test]
    fn merge_paths_keeps_login_entries_first() {
        assert_eq!(
            merge_paths("/opt/homebrew/bin:/bin", "/bin:/usr/bin:/custom/bin"),
            "/opt/homebrew/bin:/bin:/usr/bin:/custom/bin"
        );
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn command_hooks_repair_path_when_enabled() {
        let tmp = tempfile::tempdir().unwrap();
        let login_bin = tmp.path().join("login-bin");
        std::fs::create_dir(&login_bin).unwrap();

        let fake_shell = tmp.path().join("fake-login-shell");
        std::fs::write(
            &fake_shell,
            "#!/bin/sh\nprintf '%s\\n' \"$FAKE_LOGIN_PATH\"\n",
        )
        .unwrap();
        let helper = login_bin.join("hook-visible-tool");
        std::fs::write(&helper, "#!/bin/sh\nprintf 'hook-visible-tool-ran'\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for path in [&fake_shell, &helper] {
                let mut perms = std::fs::metadata(path).unwrap().permissions();
                perms.set_mode(0o755);
                std::fs::set_permissions(path, perms).unwrap();
            }
        }

        let fake_shell = fake_shell.to_string_lossy().into_owned();
        let fake_login_path = format!("{}:/usr/bin:/bin", login_bin.display());
        let _guard = env_lock::lock_env([
            ("GOOSE_SHELL", Some(fake_shell.as_str())),
            ("FAKE_LOGIN_PATH", Some(fake_login_path.as_str())),
            (
                "PATH",
                Some(
                    "/Applications/Goose.app/Contents/Resources/bin:/usr/bin:/bin:/usr/sbin:/sbin",
                ),
            ),
        ]);

        let run = run_command_hook(
            "hook-visible-tool",
            tmp.path(),
            "{}",
            Duration::from_secs(5),
            true,
        )
        .await
        .unwrap();

        assert!(run.output.status.success());
        assert!(run.stdin_delivered, "a small payload must reach the hook");
        assert_eq!(
            String::from_utf8_lossy(&run.output.stdout),
            "hook-visible-tool-ran"
        );
    }

    #[tokio::test]
    async fn matcher_filters_by_tool_name() {
        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join("ran.txt");
        let hooks = format!(
            r#"{{"hooks":{{"PreToolUse":[{{"matcher":"developer__shell","hooks":[{{"type":"command","command":"touch {}"}}]}}]}}}}"#,
            marker.to_string_lossy(),
        );
        let root = write_plugin(tmp.path(), "p", &hooks);
        let mgr = make_manager(vec![DiscoveredPlugin {
            name: "p".into(),
            root,
            scope: PluginScope::User,
        }]);

        // Non-matching tool: marker not created.
        mgr.emit(
            HookEvent::PreToolUse,
            HookContext::new(HookEvent::PreToolUse, "s").with_tool("other__tool", None),
        )
        .await;
        assert!(!marker.exists());

        // Matching tool: marker created.
        mgr.emit(
            HookEvent::PreToolUse,
            HookContext::new(HookEvent::PreToolUse, "s").with_tool("developer__shell", None),
        )
        .await;
        assert!(marker.exists());
    }

    #[test]
    fn extract_banner_from_json() {
        assert_eq!(
            extract_banner(r#"{"banner":"  🌱 hello"}"#),
            Some("  🌱 hello".to_string())
        );
    }

    #[test]
    fn extract_banner_ignores_non_json() {
        assert_eq!(extract_banner("just some text"), None);
    }

    #[test]
    fn extract_banner_ignores_json_without_banner_field() {
        assert_eq!(extract_banner(r#"{"decision":"allow"}"#), None);
    }

    #[test]
    fn extract_banner_ignores_empty_banner() {
        assert_eq!(extract_banner(r#"{"banner":""}"#), None);
    }

    #[tokio::test]
    async fn emit_collecting_banners_returns_banner_lines() {
        let tmp = tempfile::tempdir().unwrap();
        let hooks = r#"{"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"printf '{\"banner\":\"  🌱 test banner\"}'" }]}]}}"#;
        let root = write_plugin(tmp.path(), "p", hooks);
        let mgr = make_manager(vec![DiscoveredPlugin {
            name: "p".into(),
            root,
            scope: PluginScope::User,
        }]);

        let banners = mgr
            .emit_collecting_banners(
                HookEvent::SessionStart,
                HookContext::new(HookEvent::SessionStart, "s"),
            )
            .await;

        assert_eq!(banners, vec!["  🌱 test banner"]);
    }

    #[tokio::test]
    async fn emit_collecting_banners_skips_non_json_output() {
        let tmp = tempfile::tempdir().unwrap();
        let hooks =
            r#"{"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"echo hello"}]}]}}"#;
        let root = write_plugin(tmp.path(), "p", hooks);
        let mgr = make_manager(vec![DiscoveredPlugin {
            name: "p".into(),
            root,
            scope: PluginScope::User,
        }]);

        let banners = mgr
            .emit_collecting_banners(
                HookEvent::SessionStart,
                HookContext::new(HookEvent::SessionStart, "s"),
            )
            .await;

        assert!(banners.is_empty());
    }
}
