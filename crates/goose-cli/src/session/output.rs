use crate::session::builder::ExtensionFailure;
use anstream::{adapter::strip_str, eprintln, println};
use bat::WrappingMode;
use console::{measure_text_width, style, Color, StyledObject, Term};
use goose::config::Config;
use goose::conversation::message::{
    ActionRequiredData, Message, MessageContent, SystemNotificationContent, SystemNotificationType,
    ToolNameParts, ToolRequest, ToolResponse,
};
use goose::providers::canonical_cost::estimate_model_cost;
#[cfg(target_os = "windows")]
use goose::subprocess::SubprocessExt;
use goose::utils::safe_truncate;
use goose_providers::conversation::token_usage::Usage;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use rmcp::model::{CallToolRequestParams, JsonObject, PromptArgument, Role};
use serde_json::Value;
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::fmt::Display;
use std::io::{Error, IsTerminal, Write};
use std::path::Path;
use std::time::Duration;

use super::streaming_buffer::MarkdownBuffer;

pub const DEFAULT_MIN_PRIORITY: f32 = 0.0;
pub const DEFAULT_CLI_LIGHT_THEME: &str = "GitHub";
pub const DEFAULT_CLI_DARK_THEME: &str = "zenburn";
const OUTPUT_TOKEN_LIMIT_WARNING: &str =
    "Warning: Response reached the model's output-token limit and may be incomplete.";

fn accent<T: Display>(value: T) -> StyledObject<T> {
    style(value).cyan()
}

fn success<T: Display>(value: T) -> StyledObject<T> {
    style(value).green()
}

fn warning<T: Display>(value: T) -> StyledObject<T> {
    style(value).yellow()
}

fn danger<T: Display>(value: T) -> StyledObject<T> {
    style(value).red()
}

// Re-export theme for use in main
#[derive(Clone, Copy)]
pub enum Theme {
    Light,
    Dark,
    Ansi,
}

impl Theme {
    fn as_str(&self) -> String {
        match self {
            Theme::Light => Config::global()
                .get_param::<String>("GOOSE_CLI_LIGHT_THEME")
                .unwrap_or(DEFAULT_CLI_LIGHT_THEME.to_string()),
            Theme::Dark => Config::global()
                .get_param::<String>("GOOSE_CLI_DARK_THEME")
                .unwrap_or(DEFAULT_CLI_DARK_THEME.to_string()),
            Theme::Ansi => "base16".to_string(),
        }
    }

    fn from_config_str(val: &str) -> Self {
        if val.eq_ignore_ascii_case("light") {
            Theme::Light
        } else if val.eq_ignore_ascii_case("ansi") {
            Theme::Ansi
        } else {
            Theme::Dark
        }
    }

    fn as_config_string(&self) -> String {
        match self {
            Theme::Light => "light".to_string(),
            Theme::Dark => "dark".to_string(),
            Theme::Ansi => "ansi".to_string(),
        }
    }
}

thread_local! {
    static CURRENT_THEME: RefCell<Theme> = RefCell::new(
        std::env::var("GOOSE_CLI_THEME").ok()
            .map(|val| Theme::from_config_str(&val))
            .unwrap_or_else(||
                Config::global().get_param::<String>("GOOSE_CLI_THEME").ok()
                    .map(|val| Theme::from_config_str(&val))
                    .unwrap_or(Theme::Ansi)
            )
    );
    static SHOW_FULL_TOOL_OUTPUT: RefCell<bool> = RefCell::new(
        Config::global().get_param::<bool>("GOOSE_SHOW_FULL_OUTPUT").unwrap_or(false)
    );
}

pub fn set_theme(theme: Theme) {
    let config = Config::global();
    config
        .set_param("GOOSE_CLI_THEME", theme.as_config_string())
        .expect("Failed to set theme");
    CURRENT_THEME.with(|t| *t.borrow_mut() = theme);

    let config = Config::global();
    let theme_str = match theme {
        Theme::Light => "light",
        Theme::Dark => "dark",
        Theme::Ansi => "ansi",
    };

    if let Err(e) = config.set_param("GOOSE_CLI_THEME", theme_str) {
        eprintln!("Failed to save theme setting to config: {}", e);
    }
}

/// Ring the terminal bell so an unfocused terminal can badge or chime.
/// Opt-in via `GOOSE_CLI_BELL=true` (environment or config); terminals
/// decide how to surface it, typically only when the window lacks focus.
pub fn emit_attention_bell() {
    if !bell_enabled() {
        return;
    }
    let mut stdout = std::io::stdout();
    if !stdout.is_terminal() {
        return;
    }
    let _ = stdout.write_all(b"\x07");
    let _ = stdout.flush();
}

fn bell_enabled() -> bool {
    Config::global()
        .get_param::<bool>("GOOSE_CLI_BELL")
        .unwrap_or(false)
}

pub fn get_theme() -> Theme {
    CURRENT_THEME.with(|t| *t.borrow())
}

pub fn toggle_full_tool_output() -> bool {
    SHOW_FULL_TOOL_OUTPUT.with(|s| {
        let mut val = s.borrow_mut();
        *val = !*val;
        *val
    })
}

pub fn get_show_full_tool_output() -> bool {
    SHOW_FULL_TOOL_OUTPUT.with(|s| *s.borrow())
}

// Simple wrapper around spinner to manage its state
#[derive(Default)]
pub struct ThinkingIndicator {
    spinner: Option<cliclack::ProgressBar>,
}

impl ThinkingIndicator {
    pub fn show(&mut self) {
        let spinner = cliclack::spinner();
        let hint = style("(Ctrl+C to interrupt)").dim();
        if Config::global()
            .get_param("RANDOM_THINKING_MESSAGES")
            .unwrap_or(true)
        {
            spinner.start(format!(
                "{}...  {}",
                super::thinking::get_random_thinking_message(),
                hint,
            ));
        } else {
            spinner.start(format!("Thinking...  {}", hint));
        }
        self.spinner = Some(spinner);
    }

    pub fn hide(&mut self) {
        if let Some(spinner) = self.spinner.take() {
            spinner.stop("");
        }
    }

    pub fn is_shown(&self) -> bool {
        self.spinner.is_some()
    }
}

#[derive(Debug, Clone)]
pub struct PromptInfo {
    pub name: String,
    pub description: Option<String>,
    pub arguments: Option<Vec<PromptArgument>>,
    pub extension: Option<String>,
}

// Global thinking indicator
thread_local! {
    static THINKING: RefCell<ThinkingIndicator> = RefCell::new(ThinkingIndicator::default());
}

pub fn show_thinking() {
    if std::io::stdout().is_terminal() {
        THINKING.with(|t| t.borrow_mut().show());
    }
}

pub fn hide_thinking() {
    if std::io::stdout().is_terminal() {
        THINKING.with(|t| t.borrow_mut().hide());
    }
}

pub fn show_loading_extensions_background() {
    eprintln!(
        "  {}",
        style("⏳ loading extensions in background...").dim()
    );
}

pub fn show_waiting_for_extensions() {
    eprintln!(
        "  {}",
        style("⏳ waiting for extensions to finish loading...").dim()
    );
}

pub fn show_extensions_ready() {
    eprintln!("  {}", style("✓ extensions ready").green());
}

pub fn show_extension_failures(failures: &[ExtensionFailure]) {
    for failure in failures {
        match failure.label.as_deref() {
            None => {
                eprintln!(
                    "{}",
                    style(format!(
                        "  ⚠ Failed to start extensions ({})",
                        failure.error
                    ))
                    .yellow()
                );
            }
            Some(label) => {
                eprintln!(
                    "{}",
                    style(format!(
                        "  ⚠ Failed to start extension '{}' ({}), continuing without it",
                        label, failure.error
                    ))
                    .yellow()
                );
                eprintln!(
                    "{}",
                    style(format!(
                        "    Hint: ask goose to help debug the '{}' extension",
                        label
                    ))
                    .dim()
                );
            }
        }
    }
}

pub fn run_status_hook(status: &str) {
    if let Ok(hook) = Config::global().get_param::<String>("GOOSE_STATUS_HOOK") {
        let status = status.to_string();
        std::thread::spawn(move || {
            #[cfg(target_os = "windows")]
            let result = std::process::Command::new("cmd")
                .arg("/C")
                .arg(format!("{} {}", hook, status))
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .set_no_window()
                .status();

            #[cfg(not(target_os = "windows"))]
            let result = std::process::Command::new("sh")
                .arg("-c")
                .arg(format!("{} {}", hook, status))
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();

            let _ = result;
        });
    }
}

pub fn is_showing_thinking() -> bool {
    THINKING.with(|t| t.borrow().is_shown())
}

pub fn set_thinking_message(s: &String) {
    if std::io::stdout().is_terminal() {
        THINKING.with(|t| {
            if let Some(spinner) = t.borrow_mut().spinner.as_mut() {
                spinner.set_message(s);
            }
        });
    }
}

pub fn render_message(message: &Message, debug: bool) {
    if !message.is_user_visible() {
        return;
    }
    let message = message.user_visible_content();
    let theme = get_theme();

    for content in &message.content {
        match content {
            MessageContent::ActionRequired(action) => match &action.data {
                ActionRequiredData::ToolConfirmation { tool_name, .. } => {
                    println!("action_required(tool_confirmation): {}", tool_name)
                }
                ActionRequiredData::Elicitation { message, .. } => {
                    println!("action_required(elicitation): {}", message)
                }
                ActionRequiredData::ElicitationResponse { id, .. } => {
                    println!("action_required(elicitation_response): {}", id)
                }
                ActionRequiredData::ToolConfirmationResponse { id, .. } => {
                    println!("action_required(tool_confirmation_response): {}", id)
                }
            },
            MessageContent::Text(text) => print_markdown(&text.text, theme),
            MessageContent::ToolRequest(req) => render_tool_request(req, theme, debug),
            MessageContent::ToolResponse(resp) => render_tool_response(resp, debug),
            MessageContent::Image(image) => {
                println!("Image: [data: {}, type: {}]", image.data, image.mime_type);
            }
            MessageContent::Thinking(t) => render_thinking(&t.thinking, theme),
            MessageContent::RedactedThinking(_) => {
                println!("\n{}", style("Thinking:").dim().italic());
                print_markdown("Thinking was redacted", theme);
            }
            MessageContent::SystemNotification(notification) => {
                match notification.notification_type {
                    SystemNotificationType::ThinkingMessage
                    | SystemNotificationType::ProgressMessage => {
                        show_thinking();
                        set_thinking_message(&notification.msg);
                    }
                    SystemNotificationType::InlineMessage => {
                        hide_thinking();
                        println!("\n{} {}", style("·").dim(), &notification.msg);
                    }
                    SystemNotificationType::CreditsExhausted => {
                        render_credits_exhausted_notification(notification);
                    }
                }
            }
            MessageContent::Error(error) => {
                hide_thinking();
                println!("\n{} {}", danger("error:").bold(), &error.message);
            }
            _ => {
                eprintln!("WARNING: Message content type could not be rendered");
            }
        }
    }

    if reached_output_token_limit(&message) {
        render_output_token_limit_warning();
    }
    let _ = std::io::stdout().flush();
}

/// Render a streaming message, using a buffer to accumulate text content
/// and only render when markdown constructs are complete.
pub fn render_message_streaming(
    message: &Message,
    buffer: &mut MarkdownBuffer,
    thinking_header_shown: &mut bool,
    debug: bool,
) {
    if !message.is_user_visible() {
        return;
    }
    let message = message.user_visible_content();
    let theme = get_theme();

    for content in &message.content {
        if !matches!(content, MessageContent::Thinking(_)) {
            if *thinking_header_shown {
                println!();
            }
            *thinking_header_shown = false;
        }

        match content {
            MessageContent::Text(text) => {
                if let Some(safe_content) = buffer.push(&text.text) {
                    print_markdown(&safe_content, theme);
                }
            }
            MessageContent::ToolRequest(req) => {
                flush_markdown_buffer(buffer, theme);
                render_tool_request(req, theme, debug);
            }
            MessageContent::ToolResponse(resp) => {
                flush_markdown_buffer(buffer, theme);
                render_tool_response(resp, debug);
            }
            MessageContent::ActionRequired(action) => {
                flush_markdown_buffer(buffer, theme);
                match &action.data {
                    ActionRequiredData::ToolConfirmation { tool_name, .. } => {
                        println!("action_required(tool_confirmation): {}", tool_name)
                    }
                    ActionRequiredData::Elicitation { message, .. } => {
                        println!("action_required(elicitation): {}", message)
                    }
                    ActionRequiredData::ElicitationResponse { id, .. } => {
                        println!("action_required(elicitation_response): {}", id)
                    }
                    ActionRequiredData::ToolConfirmationResponse { id, .. } => {
                        println!("action_required(tool_confirmation_response): {}", id)
                    }
                }
            }
            MessageContent::Image(image) => {
                flush_markdown_buffer(buffer, theme);
                println!("Image: [data: {}, type: {}]", image.data, image.mime_type);
            }
            MessageContent::Thinking(t) => {
                render_thinking_streaming(&t.thinking, buffer, thinking_header_shown, theme);
            }
            MessageContent::RedactedThinking(_) => {
                flush_markdown_buffer(buffer, theme);
                println!("\n{}", style("Thinking:").dim().italic());
                print_markdown("Thinking was redacted", theme);
            }
            MessageContent::SystemNotification(notification) => {
                match notification.notification_type {
                    SystemNotificationType::ThinkingMessage
                    | SystemNotificationType::ProgressMessage => {
                        show_thinking();
                        set_thinking_message(&notification.msg);
                    }
                    SystemNotificationType::InlineMessage => {
                        flush_markdown_buffer(buffer, theme);
                        hide_thinking();
                        println!("\n{} {}", style("·").dim(), &notification.msg);
                    }
                    SystemNotificationType::CreditsExhausted => {
                        flush_markdown_buffer(buffer, theme);
                        render_credits_exhausted_notification(notification);
                    }
                }
            }
            MessageContent::Error(error) => {
                flush_markdown_buffer(buffer, theme);
                hide_thinking();
                println!("\n{} {}", danger("error:").bold(), &error.message);
            }
            _ => {
                flush_markdown_buffer(buffer, theme);
                eprintln!("WARNING: Message content type could not be rendered");
            }
        }
    }

    if reached_output_token_limit(&message) {
        flush_markdown_buffer(buffer, theme);
        render_output_token_limit_warning();
    }
    let _ = std::io::stdout().flush();
}

fn reached_output_token_limit(message: &Message) -> bool {
    message.role == Role::Assistant && message.metadata.output_token_limit_reached
}

fn render_output_token_limit_warning() {
    println!("\n{}", warning(OUTPUT_TOKEN_LIMIT_WARNING));
}

fn render_credits_exhausted_notification(notification: &SystemNotificationContent) {
    hide_thinking();
    println!("\n{} {}", warning("warning:").bold(), &notification.msg);

    if let Some(url) = notification
        .data
        .as_ref()
        .and_then(|d| d.get("top_up_url"))
        .and_then(|v| v.as_str())
    {
        println!("{} {}", style("top up:").dim(), accent(url));
    }
}

pub fn get_credits_top_up_url(message: &Message) -> Option<String> {
    message.content.iter().find_map(|content| {
        let MessageContent::SystemNotification(notification) = content else {
            return None;
        };
        if notification.notification_type != SystemNotificationType::CreditsExhausted {
            return None;
        }
        notification
            .data
            .as_ref()
            .and_then(|d| d.get("top_up_url"))
            .and_then(|v| v.as_str())
            .map(str::to_string)
    })
}

pub fn flush_markdown_buffer(buffer: &mut MarkdownBuffer, theme: Theme) {
    let remaining = buffer.flush();
    if !remaining.is_empty() {
        print_markdown(&remaining, theme);
    }
}

pub fn flush_markdown_buffer_current_theme(buffer: &mut MarkdownBuffer) {
    flush_markdown_buffer(buffer, get_theme());
}

pub fn render_text(text: &str, color: Option<Color>, dim: bool) {
    render_text_no_newlines(format!("\n{}\n\n", text).as_str(), color, dim);
}

pub fn render_text_no_newlines(text: &str, color: Option<Color>, dim: bool) {
    if !std::io::stdout().is_terminal() {
        println!("{}", text);
        return;
    }
    let mut styled_text = style(text);
    if dim {
        styled_text = styled_text.dim();
    }
    if let Some(color) = color {
        styled_text = styled_text.fg(color);
    }
    print!("{}", styled_text);
}

pub fn render_enter_plan_mode() {
    println!(
        "\n{} {}\n",
        accent("Entering plan mode.").bold(),
        style("You can provide instructions to create a plan and then act on it. To exit early, type /endplan")
            .dim()
    );
}

pub fn render_act_on_plan() {
    println!(
        "\n{}\n",
        accent("Exiting plan mode and acting on the above plan").bold(),
    );
}

pub fn render_exit_plan_mode() {
    println!("\n{}\n", accent("Exiting plan mode.").bold());
}

pub fn goose_mode_message(text: &str) {
    println!("\n{} {}", accent("mode:"), text);
}

fn should_show_thinking() -> bool {
    Config::global()
        .get_param::<bool>("GOOSE_CLI_SHOW_THINKING")
        .unwrap_or(false)
        && std::io::stdout().is_terminal()
}

fn render_thinking(text: &str, theme: Theme) {
    if should_show_thinking() && !text.is_empty() {
        println!("\n{}", style("Thinking:").dim().italic());
        print_markdown(text, theme);
    }
}

fn render_thinking_streaming(
    text: &str,
    buffer: &mut MarkdownBuffer,
    header_shown: &mut bool,
    theme: Theme,
) {
    if should_show_thinking() && !text.is_empty() {
        flush_markdown_buffer(buffer, theme);
        if !*header_shown {
            println!("\n{}", style("Thinking:").dim().italic());
            *header_shown = true;
        }
        print!("{}", style(text).dim());
        let _ = std::io::stdout().flush();
    }
}

fn render_tool_request(req: &ToolRequest, theme: Theme, debug: bool) {
    match &req.tool_call {
        Ok(call) => match call.name.to_string().as_str() {
            name if is_shell_tool_name(name) => render_shell_request(call, debug),
            name if is_file_tool_name(name) => render_text_editor_request(call, debug),
            "execute_typescript" | "execute_code" => render_execute_code_request(call, debug),
            "delegate" => render_delegate_request(call, debug),
            "subagent" => render_delegate_request(call, debug),
            "todo__write" => render_todo_request(call, debug),
            "load" => {}
            _ => render_default_request(call, debug),
        },
        Err(e) => print_markdown(&e.to_string(), theme),
    }
}

fn render_tool_response(resp: &ToolResponse, debug: bool) {
    let config = Config::global();

    match &resp.tool_result {
        Ok(result) => {
            for content in &result.content {
                let annotations = match content {
                    rmcp::model::ContentBlock::Text(t) => t.annotations.as_ref(),
                    rmcp::model::ContentBlock::Image(i) => i.annotations.as_ref(),
                    rmcp::model::ContentBlock::Audio(a) => a.annotations.as_ref(),
                    rmcp::model::ContentBlock::Resource(r) => r.annotations.as_ref(),
                    rmcp::model::ContentBlock::ResourceLink(r) => r.annotations.as_ref(),
                    _ => None,
                };
                if let Some(audience) = annotations.and_then(|a| a.audience.as_ref()) {
                    if !audience.contains(&rmcp::model::Role::User) {
                        continue;
                    }
                }

                let min_priority = config
                    .get_param::<f32>("GOOSE_CLI_MIN_PRIORITY")
                    .ok()
                    .unwrap_or(DEFAULT_MIN_PRIORITY);

                let priority = annotations.and_then(|a| a.priority);
                if priority.is_some_and(|priority| priority < min_priority)
                    || (priority.is_none() && !debug)
                {
                    continue;
                }

                if debug {
                    println!("{:#?}", content);
                } else if let Some(text) = content.as_text() {
                    print_tool_output(&text.text);
                }
            }
        }
        Err(e) => {
            println!("    {}", style(e.to_string()).red().dim());
        }
    }
}

pub(super) fn sanitize_terminal_line(line: &str) -> String {
    strip_str(line)
        .flat_map(str::chars)
        .filter(|character| *character == '\t' || !character.is_control())
        .collect()
}

fn sanitize_tool_confirmation_line(line: &str) -> String {
    let mut sanitized = String::with_capacity(line.len());
    for character in sanitize_terminal_line(line).chars() {
        if matches!(
            character,
            '\u{061c}'
                | '\u{200e}'
                | '\u{200f}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2066}'..='\u{2069}'
        ) {
            sanitized.push_str(&format!("\\u{:04x}", character as u32));
        } else {
            sanitized.push(character);
        }
    }
    sanitized
}

fn sanitize_tool_confirmation_text(text: &str) -> String {
    text.split('\n')
        .map(sanitize_tool_confirmation_line)
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn format_tool_confirmation(tool_name: &str, arguments: &JsonObject) -> String {
    let mut request = JsonObject::new();
    request.insert(
        "tool_name".to_string(),
        Value::String(tool_name.to_string()),
    );
    request.insert("arguments".to_string(), Value::Object(arguments.clone()));

    serde_json::to_string_pretty(&Value::Object(request))
        .expect("tool confirmation request must serialize")
        .lines()
        .map(sanitize_tool_confirmation_line)
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn render_tool_confirmation(
    tool_name: &str,
    arguments: &JsonObject,
    security_prompt: Option<&str>,
) {
    eprintln!();
    if let Some(security_prompt) = security_prompt {
        eprintln!("  {}", style("Provider-provided approval notice").bold());
        for line in sanitize_tool_confirmation_text(security_prompt).split('\n') {
            eprintln!("    {}", style(line).dim());
        }
        eprintln!();
    }
    eprintln!("  {}", style("Tool approval request").bold());
    for line in format_tool_confirmation(tool_name, arguments).lines() {
        eprintln!("    {}", style(line).dim());
    }
    eprintln!();
}

fn print_tool_output_line(line: &str) {
    println!("    {}", style(sanitize_terminal_line(line)).dim());
}

fn print_tool_output(text: &str) {
    if text.is_empty() {
        return;
    }
    if !std::io::stdout().is_terminal() {
        print!("{}", text);
        return;
    }
    let max_lines = if get_show_full_tool_output() {
        usize::MAX
    } else {
        20
    };
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= max_lines {
        for line in &lines {
            print_tool_output_line(line);
        }
    } else {
        let head = max_lines / 2;
        let tail = max_lines - head;
        for line in &lines[..head] {
            print_tool_output_line(line);
        }
        println!(
            "    {}",
            style(format!(
                "... ({} lines hidden, /toggle to show all)",
                lines.len() - head - tail
            ))
            .dim()
            .italic()
        );
        for line in &lines[lines.len() - tail..] {
            print_tool_output_line(line);
        }
    }
}

fn is_shell_tool_name(name: &str) -> bool {
    matches!(name, "shell")
}

fn is_file_tool_name(name: &str) -> bool {
    matches!(name, "write" | "edit")
}

pub fn render_error(message: &str) {
    println!("\n  {} {}\n", danger("error:").bold(), message);
}

pub fn render_prompts(prompts: &HashMap<String, Vec<String>>) {
    println!();
    for (extension, prompts) in prompts {
        println!(" {}", accent(extension));
        for prompt in prompts {
            println!("  - {}", style(prompt).cyan());
        }
    }
    println!();
}

pub fn render_prompt_info(info: &PromptInfo) {
    println!();
    if let Some(ext) = &info.extension {
        println!(" {}: {}", accent("Extension"), ext);
    }
    println!(" Prompt: {}", style(&info.name).cyan().bold());
    if let Some(desc) = &info.description {
        println!("\n {}", desc);
    }
    render_arguments(info);
    println!();
}

fn render_arguments(info: &PromptInfo) {
    if let Some(args) = &info.arguments {
        println!("\n Arguments:");
        for arg in args {
            let required = arg.required.unwrap_or(false);
            let req_str = if required {
                style("(required)").bold()
            } else {
                style("(optional)").dim()
            };

            println!(
                "  {} {} {}",
                accent(&arg.name),
                req_str,
                arg.description.as_deref().unwrap_or("")
            );
        }
    }
}

pub fn render_extension_success(name: &str) {
    println!();
    println!("  {} extension `{}`", success("added"), accent(name),);
    println!();
}

pub fn render_extension_error(name: &str, error: &str) {
    println!();
    println!("  {} to add extension {}", danger("failed"), danger(name));
    println!();
    println!("{}", style(error).dim());
    println!();
}

pub fn render_builtin_success(names: &str) {
    println!();
    println!(
        "  {} builtin{}: {}",
        success("added"),
        if names.contains(',') { "s" } else { "" },
        accent(names)
    );
    println!();
}

pub fn render_builtin_error(names: &str, error: &str) {
    println!();
    println!(
        "  {} to add builtin{}: {}",
        danger("failed"),
        if names.contains(',') { "s" } else { "" },
        danger(names)
    );
    println!();
    println!("{}", style(error).dim());
    println!();
}

fn render_text_editor_request(call: &CallToolRequestParams, debug: bool) {
    print_tool_header(call);

    if let Some(args) = &call.arguments {
        if let Some(Value::String(path)) = args.get("path") {
            println!(
                "    {} {}",
                style("path").dim(),
                style(shorten_path(path, debug)).dim()
            );
        }

        if let Some(args) = &call.arguments {
            let mut other_args = serde_json::Map::new();
            for (k, v) in args {
                if k != "path" {
                    other_args.insert(k.clone(), v.clone());
                }
            }
            if !other_args.is_empty() {
                print_params(&Some(other_args), 1, debug);
            }
        }
    }
    println!();
}

fn render_shell_request(call: &CallToolRequestParams, debug: bool) {
    print_tool_header(call);
    print_params(&call.arguments, 1, debug);
    println!();
}

fn render_execute_code_request(call: &CallToolRequestParams, debug: bool) {
    let tool_graph = call
        .arguments
        .as_ref()
        .and_then(|args| args.get("tool_graph"))
        .and_then(Value::as_array)
        .filter(|arr| !arr.is_empty());

    let Some(tool_graph) = tool_graph else {
        return render_default_request(call, debug);
    };

    let count = tool_graph.len();
    let plural = if count == 1 { "" } else { "s" };
    println!();
    println!(
        "  {} {} {} tool call{}",
        style("▸").dim(),
        style("execute").dim(),
        style(count).dim(),
        plural,
    );

    for (i, node) in tool_graph.iter().filter_map(Value::as_object).enumerate() {
        let tool = node
            .get("tool")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let desc = node
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("");
        let deps: Vec<_> = node
            .get("depends_on")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_u64)
            .map(|d| (d + 1).to_string())
            .collect();
        let deps_str = if deps.is_empty() {
            String::new()
        } else {
            format!(" (uses {})", deps.join(", "))
        };
        println!(
            "    {}. {} {}{}",
            style(i + 1).dim(),
            style(tool).dim(),
            style(desc).dim(),
            style(deps_str).dim()
        );
    }

    let code = call
        .arguments
        .as_ref()
        .and_then(|args| args.get("code"))
        .and_then(Value::as_str)
        .filter(|c| !c.is_empty());
    if code.is_some_and(|_| debug) {
        println!("{}", code.unwrap_or_default());
    }

    println!();
}

fn render_delegate_request(call: &CallToolRequestParams, debug: bool) {
    print_tool_header(call);

    if let Some(args) = &call.arguments {
        if let Some(Value::String(source)) = args.get("source") {
            println!("    {} {}", style("source").dim(), style(source).dim());
        }

        if let Some(Value::String(instructions)) = args.get("instructions") {
            let display = if instructions.len() > 100 && !debug {
                safe_truncate(instructions, 100)
            } else {
                instructions.clone()
            };
            println!(
                "    {} {}",
                style("instructions").dim(),
                style(display).dim()
            );
        }

        if let Some(Value::Object(params)) = args.get("parameters") {
            println!("    {}:", style("parameters").dim());
            print_params(&Some(params.clone()), 2, debug);
        }

        let skip_keys = ["source", "instructions", "parameters"];
        let mut other_args = serde_json::Map::new();
        for (k, v) in args {
            if !skip_keys.contains(&k.as_str()) {
                other_args.insert(k.clone(), v.clone());
            }
        }
        if !other_args.is_empty() {
            print_params(&Some(other_args), 1, debug);
        }
    }

    println!();
}

fn render_todo_request(call: &CallToolRequestParams, _debug: bool) {
    print_tool_header(call);

    if let Some(args) = &call.arguments {
        if let Some(Value::String(content)) = args.get("content") {
            println!("    {} {}", style("content").dim(), style(content).dim());
        }
    }
    println!();
}

fn render_default_request(call: &CallToolRequestParams, debug: bool) {
    print_tool_header(call);
    print_params(&call.arguments, 1, debug);
    println!();
}

fn extension_display_name(name: &str) -> &str {
    match name {
        "code_execution" => "Code Mode",
        _ => name,
    }
}

pub fn format_subagent_tool_call_message(subagent_id: &str, tool_name: &str) -> String {
    let short_id = subagent_id.rsplit('_').next().unwrap_or(subagent_id);
    let parts = ToolNameParts::from(tool_name);

    match parts.extension_name {
        Some(extension_name) => format!(
            "[subagent:{}] {} | {}",
            short_id,
            parts.tool_name,
            extension_display_name(extension_name)
        ),
        None => format!("[subagent:{}] {}", short_id, parts.tool_name),
    }
}

pub fn render_subagent_tool_call(
    subagent_id: &str,
    tool_name: &str,
    arguments: Option<&JsonObject>,
    debug: bool,
) {
    if tool_name == "code_execution__execute_typescript" {
        let tool_graph = arguments
            .and_then(|args| args.get("tool_graph"))
            .and_then(Value::as_array)
            .filter(|arr| !arr.is_empty());
        if let Some(tool_graph) = tool_graph {
            return render_subagent_tool_graph(subagent_id, tool_graph);
        }
    }
    let tool_header = format!(
        "  {} {}",
        style("▸").dim(),
        style(format_subagent_tool_call_message(subagent_id, tool_name)).dim(),
    );
    println!();
    println!("{}", tool_header);
    print_params(&arguments.cloned(), 1, debug);
    println!();
}

fn render_subagent_tool_graph(subagent_id: &str, tool_graph: &[Value]) {
    let short_id = subagent_id.rsplit('_').next().unwrap_or(subagent_id);
    let count = tool_graph.len();
    let plural = if count == 1 { "" } else { "s" };
    println!();
    println!(
        "  {} {} {} {} tool call{}",
        style("▸").dim(),
        style(format!("[subagent:{}]", short_id)).dim(),
        style("execute_typescript").dim(),
        style(count).dim(),
        plural,
    );

    for (i, node) in tool_graph.iter().filter_map(Value::as_object).enumerate() {
        let tool = node
            .get("tool")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let desc = node
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("");
        let deps: Vec<_> = node
            .get("depends_on")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_u64)
            .map(|d| (d + 1).to_string())
            .collect();
        let deps_str = if deps.is_empty() {
            String::new()
        } else {
            format!(" (uses {})", deps.join(", "))
        };
        println!(
            "    {}. {} {}{}",
            style(i + 1).dim(),
            style(tool).dim(),
            style(desc).dim(),
            style(deps_str).dim()
        );
    }
    println!();
}

// Helper functions

fn print_tool_header(call: &CallToolRequestParams) {
    let parts = ToolNameParts::from(call.name.as_ref());
    let tool_header = match parts.extension_name {
        Some(extension_name) => format!(
            "  {} {} {}",
            style("▸").dim(),
            style(parts.tool_name).dim(),
            style(extension_display_name(extension_name))
                .magenta()
                .dim(),
        ),
        None => format!("  {} {}", style("▸").dim(), style(parts.tool_name).dim()),
    };
    println!();
    println!("  {}", style("─".repeat(40)).dim());
    println!("{}", tool_header);
}

// Respect NO_COLOR, as https://crates.io/crates/console already does
pub fn env_no_color() -> bool {
    // if NO_COLOR is defined at all disable colors
    std::env::var_os("NO_COLOR").is_none()
}

fn print_markdown(content: &str, theme: Theme) {
    if std::io::stdout().is_terminal() {
        if let Some((before, table, after)) = extract_markdown_table(content) {
            if !before.is_empty() {
                print_markdown_raw(&before, theme);
            }
            print_table(&table, theme);
            if !after.is_empty() {
                print_markdown(after, theme);
            }
        } else {
            print_markdown_raw(content, theme);
        }
    } else {
        print!("{}", content);
    }
}

/// Renders markdown content using bat (no table processing)
///
/// The printer is cached per thread because `PrettyPrinter::new()`
/// deserializes bat's bundled syntax/theme assets; `print()` drains the
/// queued inputs but leaves the printer reusable.
fn print_markdown_raw(content: &str, theme: Theme) {
    use std::cell::RefCell;
    thread_local! {
        static PRINTER: RefCell<bat::PrettyPrinter<'static>> =
            RefCell::new(bat::PrettyPrinter::new());
    }
    PRINTER.with(|printer| {
        printer
            .borrow_mut()
            .input(bat::Input::from_reader(Box::new(std::io::Cursor::new(
                content.as_bytes().to_vec(),
            ))))
            .theme(theme.as_str())
            .colored_output(env_no_color())
            .language("Markdown")
            .wrapping_mode(WrappingMode::NoWrapping(true))
            .print()
            .unwrap();
    });
}

fn extract_markdown_table(content: &str) -> Option<(String, Vec<&str>, &str)> {
    let lines: Vec<&str> = content.lines().collect();

    // Track newline positions for safe slicing later
    let newline_indices: Vec<usize> = content
        .bytes()
        .enumerate()
        .filter_map(|(i, b)| if b == b'\n' { Some(i) } else { None })
        .collect();

    // Skip tables inside code blocks
    let mut in_code_block = false;
    let mut table_start = None;
    let mut table_end = None;

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();

        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_code_block = !in_code_block;
            continue;
        }

        if in_code_block {
            continue;
        }

        if trimmed.starts_with('|') && trimmed.ends_with('|') {
            if table_start.is_none() {
                table_start = Some(i);
            }
            table_end = Some(i);
        } else if table_start.is_some() {
            break;
        }
    }

    let start = table_start?;
    let end = table_end?;

    // Need at least header + separator (2 rows minimum)
    if end < start + 1 {
        return None;
    }

    // Require separator to be the second row with proper format
    let separator_line = lines.get(start + 1)?;
    let is_valid_separator = separator_line.trim().starts_with('|')
        && separator_line.trim().ends_with('|')
        && separator_line
            .trim()
            .trim_matches('|')
            .split('|')
            .all(|cell| {
                let t = cell.trim();
                !t.is_empty() && t.chars().all(|c| c == '-' || c == ':' || c == ' ')
            });

    if !is_valid_separator {
        return None;
    }

    let before = lines[..start].join("\n");
    let before = if before.is_empty() {
        before
    } else {
        before + "\n"
    };
    let table = lines[start..=end].to_vec();

    let after = if end + 1 >= lines.len() {
        ""
    } else if let Some(&newline_pos) = newline_indices.get(end) {
        content.get(newline_pos + 1..).unwrap_or("")
    } else {
        ""
    };

    Some((before, table, after))
}

fn print_table(table_lines: &[&str], theme: Theme) {
    use comfy_table::{presets, Cell, CellAlignment, ContentArrangement, Table};

    let mut table = Table::new();
    table.set_content_arrangement(ContentArrangement::Dynamic);

    table.load_style(presets::ASCII_MARKDOWN);

    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut alignments: Vec<CellAlignment> = Vec::new();
    let mut separator_idx = None;

    for (i, line) in table_lines.iter().enumerate() {
        let cells: Vec<String> = line
            .trim()
            .trim_matches('|')
            .split('|')
            .map(|s| s.trim().to_string())
            .collect();

        let is_separator = cells.iter().all(|c| {
            let t = c.trim();
            t.chars().all(|ch| ch == '-' || ch == ':') && t.contains('-')
        });
        if is_separator {
            separator_idx = Some(i);
            alignments = cells
                .iter()
                .map(|c| {
                    let t = c.trim();
                    if t.starts_with(':') && t.ends_with(':') {
                        CellAlignment::Center
                    } else if t.ends_with(':') {
                        CellAlignment::Right
                    } else {
                        CellAlignment::Left
                    }
                })
                .collect();
        } else {
            rows.push(cells);
        }
    }

    if separator_idx.is_none() && !rows.is_empty() {
        alignments = vec![CellAlignment::Left; rows[0].len()];
    }

    if let Some(header) = rows.first() {
        let header_cells: Vec<Cell> = header
            .iter()
            .enumerate()
            .map(|(i, text)| {
                let cell = Cell::new(text);
                if let Some(align) = alignments.get(i) {
                    cell.set_alignment(*align)
                } else {
                    cell
                }
            })
            .collect();
        table.set_header(header_cells);
    }

    for row in rows.iter().skip(1) {
        let cells: Vec<Cell> = row
            .iter()
            .enumerate()
            .map(|(i, text)| {
                let cell = Cell::new(text);
                if let Some(align) = alignments.get(i) {
                    cell.set_alignment(*align)
                } else {
                    cell
                }
            })
            .collect();
        table.add_row(cells);
    }

    let table_str = table.to_string();
    print_markdown_raw(&table_str, theme);
}

const INDENT: &str = "    ";

fn print_value_with_prefix(prefix: &String, value: &Value, debug: bool) {
    let prefix_width = measure_text_width(prefix.as_str());
    print!("{}", prefix);
    print_value(value, debug, prefix_width)
}

fn print_value(value: &Value, debug: bool, reserve_width: usize) {
    let max_width = Term::stdout()
        .size_checked()
        .map(|(_h, w)| (w as usize).saturating_sub(reserve_width));
    let show_full = get_show_full_tool_output();
    let formatted = match value {
        Value::String(s) => match (max_width, debug || show_full) {
            (Some(w), false) if s.len() > w => style(safe_truncate(s, w)),
            _ => style(s.to_string()),
        }
        .green(),
        Value::Number(n) => style(n.to_string()).yellow(),
        Value::Bool(b) => style(b.to_string()).yellow(),
        Value::Null => style("null".to_string()).dim(),
        _ => unreachable!(),
    };
    println!("{}", formatted);
}

fn print_params(value: &Option<JsonObject>, depth: usize, debug: bool) {
    let indent = INDENT.repeat(depth);

    if let Some(json_object) = value {
        for (key, val) in json_object.iter() {
            match val {
                Value::Object(obj) => {
                    println!("{}{}:", indent, style(key).dim());
                    print_params(&Some(obj.clone()), depth + 1, debug);
                }
                Value::Array(arr) => {
                    // Check if all items are simple values (not objects or arrays)
                    let all_simple = arr.iter().all(|item| {
                        matches!(
                            item,
                            Value::String(_) | Value::Number(_) | Value::Bool(_) | Value::Null
                        )
                    });

                    if all_simple {
                        // Render inline for simple arrays, truncation will be handled by print_value if needed
                        let values: Vec<String> = arr
                            .iter()
                            .map(|item| match item {
                                Value::String(s) => s.clone(),
                                Value::Number(n) => n.to_string(),
                                Value::Bool(b) => b.to_string(),
                                Value::Null => "null".to_string(),
                                _ => unreachable!(),
                            })
                            .collect();
                        let joined_values = values.join(", ");
                        print_value_with_prefix(
                            &format!("{}{}: ", indent, style(key).dim()),
                            &Value::String(joined_values),
                            debug,
                        );
                    } else {
                        // Use the original multi-line format for complex arrays
                        println!("{}{}:", indent, style(key).dim());
                        for item in arr.iter() {
                            if let Value::Object(obj) = item {
                                println!("{}{}- ", indent, INDENT);
                                print_params(&Some(obj.clone()), depth + 2, debug);
                            } else {
                                println!("{}{}- {}", indent, INDENT, item);
                            }
                        }
                    }
                }
                _ => {
                    print_value_with_prefix(
                        &format!("{}{}: ", indent, style(key).dim()),
                        val,
                        debug,
                    );
                }
            }
        }
    }
}

fn shorten_path(path: &str, debug: bool) -> String {
    // In debug mode, return the full path
    if debug {
        return path.to_string();
    }

    let path = Path::new(path);

    // First try to convert to ~ if it's in home directory
    let home = etcetera::home_dir().ok();
    let path_str = if let Some(home) = home {
        if let Ok(stripped) = path.strip_prefix(home) {
            format!("~/{}", stripped.display())
        } else {
            path.display().to_string()
        }
    } else {
        path.display().to_string()
    };

    // If path is already short enough, return as is
    if path_str.len() <= 60 {
        return path_str;
    }

    let parts: Vec<_> = path_str.split('/').collect();

    // If we have 3 or fewer parts, return as is
    if parts.len() <= 3 {
        return path_str;
    }

    // Keep the first component (empty string before root / or ~) and last two components intact
    let mut shortened = vec![parts[0].to_string()];

    // Shorten middle components to their first letter
    for component in &parts[1..parts.len() - 2] {
        if !component.is_empty() {
            shortened.push(component.chars().next().unwrap_or('?').to_string());
        }
    }

    // Add the last two components
    shortened.push(parts[parts.len() - 2].to_string());
    shortened.push(parts[parts.len() - 1].to_string());

    shortened.join("/")
}

pub fn display_session_info(
    resume: bool,
    provider: &str,
    model: &str,
    session_id: &Option<String>,
) {
    set_terminal_title();

    let status = if resume {
        "resuming"
    } else if session_id.is_none() {
        "ephemeral"
    } else {
        "new session"
    };

    let model_display = model.to_string();

    let cwd_display = std::env::current_dir()
        .ok()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    // ASCII art goose with session info on the right
    println!();
    println!(
        "  {}  {} {} {} {} {}",
        style("  __( O)>").white(),
        style("●").green(),
        style(status).dim(),
        style("·").dim(),
        style(provider).dim(),
        style(&model_display).cyan(),
    );

    if let Some(id) = session_id {
        println!(
            "  {}  {} {} {}",
            style(r" \____)").white(),
            style(" ").dim(),
            style(id).dim(),
            style(format!("· {}", cwd_display)).dim(),
        );
    } else {
        println!(
            "  {}  {} {}",
            style(r" \____)").white(),
            style(" ").dim(),
            style(format!("  {}", cwd_display)).dim(),
        );
    }
    println!(
        "  {}  {}",
        style("   L L").white(),
        style("   goose is ready").white()
    );
}

fn set_terminal_title() {
    if !std::io::stdout().is_terminal() {
        return;
    }
    let dir_name = std::env::current_dir()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_default();
    // Sanitize: strip control characters (ESC, BEL, etc.) to prevent terminal escape injection
    let sanitized: String = dir_name.chars().filter(|c| !c.is_control()).collect();
    // OSC 0 sets the terminal window/tab title
    print!("\x1b]0;🪿 {}\x07", sanitized);
    let _ = std::io::stdout().flush();
}

pub fn display_banner(banners: &[String]) {
    for banner in banners {
        for line in banner.lines() {
            println!("{}", line);
        }
    }
}

pub fn display_context_usage(total_tokens: usize, context_limit: usize) {
    use console::style;

    if context_limit == 0 {
        println!(
            "  {}",
            style("context usage unavailable (context limit is 0)").dim()
        );
        return;
    }

    let percentage =
        (((total_tokens as f64 / context_limit as f64) * 100.0).round() as usize).min(100);

    let bar_width = 20;
    let filled = ((percentage as f64 / 100.0) * bar_width as f64).round() as usize;
    let empty = bar_width - filled.min(bar_width);

    let bar = format!("{}{}", "━".repeat(filled), "╌".repeat(empty));
    let colored_bar = if percentage < 50 {
        style(bar).green().dim()
    } else if percentage < 85 {
        style(bar).yellow()
    } else {
        style(bar).red()
    };

    fn format_tokens(n: usize) -> String {
        if n >= 1_000_000 {
            format!("{:.1}M", n as f64 / 1_000_000.0)
        } else if n >= 1_000 {
            format!("{:.0}k", n as f64 / 1_000.0)
        } else {
            n.to_string()
        }
    }

    println!(
        "  {} {} {}",
        colored_bar,
        style(format!("{}%", percentage)).dim(),
        style(format!(
            "{}/{}",
            format_tokens(total_tokens),
            format_tokens(context_limit)
        ))
        .dim(),
    );
}

fn estimate_cost_usd(provider: &str, model: &str, usage: &Usage) -> Option<f64> {
    estimate_model_cost(provider, model, usage)
}

/// Display cost information, if price data is available.
pub fn display_cost_usage(provider: &str, model: &str, usage: &Usage) {
    if let Some(cost) = estimate_cost_usd(provider, model, usage) {
        use console::style;
        let input_tokens = usage.input_tokens.unwrap_or(0);
        let output_tokens = usage.output_tokens.unwrap_or(0);
        let cache_read = usage.cache_read_input_tokens.unwrap_or(0);
        let cache_write = usage.cache_write_input_tokens.unwrap_or(0);

        let cache_breakdown = match (cache_read, cache_write) {
            (0, 0) => String::new(),
            (read, 0) => format!(" ({} cache read)", read),
            (0, write) => format!(" ({} cache write)", write),
            (read, write) => format!(" ({} cache read, {} cache write)", read, write),
        };

        eprintln!(
            "Cost: {} USD ({} tokens: in {}{}, out {})",
            style(format!("${:.4}", cost)).cyan(),
            input_tokens + output_tokens,
            input_tokens,
            cache_breakdown,
            output_tokens
        );
    }
}

pub struct McpSpinners {
    bars: HashMap<String, ProgressBar>,
    log_spinner: Option<ProgressBar>,
    shell_output_lines: VecDeque<String>,
    multi_bar: MultiProgress,
}

impl McpSpinners {
    pub fn new() -> Self {
        McpSpinners {
            bars: HashMap::new(),
            log_spinner: None,
            shell_output_lines: VecDeque::new(),
            multi_bar: MultiProgress::new(),
        }
    }

    pub fn log(&mut self, message: &str) {
        let spinner = self.log_spinner.get_or_insert_with(|| {
            let bar = self.multi_bar.add(
                ProgressBar::new_spinner()
                    .with_style(
                        ProgressStyle::with_template("{spinner:.green} {msg}")
                            .unwrap()
                            .tick_chars("⠋⠙⠚⠛⠓⠒⠊⠉"),
                    )
                    .with_message(message.to_string()),
            );
            bar.enable_steady_tick(Duration::from_millis(100));
            bar
        });

        spinner.set_message(message.to_string());
    }

    pub fn log_shell_output(&mut self, lines: Vec<String>, max_lines: usize) {
        let message = update_recent_lines(&mut self.shell_output_lines, lines, max_lines);
        if !message.is_empty() {
            self.log(&message);
        }
    }

    pub fn update(&mut self, token: &str, value: f64, total: Option<f64>, message: Option<&str>) {
        let bar = self.bars.entry(token.to_string()).or_insert_with(|| {
            if let Some(total) = total {
                self.multi_bar.add(
                    ProgressBar::new((total * 100_f64) as u64).with_style(
                        ProgressStyle::with_template("[{elapsed}] {bar:40} {pos:>3}/{len:3} {msg}")
                            .unwrap(),
                    ),
                )
            } else {
                self.multi_bar.add(ProgressBar::new_spinner())
            }
        });
        bar.set_position((value * 100_f64) as u64);
        if let Some(msg) = message {
            bar.set_message(msg.to_string());
        }
    }

    pub fn hide(&mut self) -> Result<(), Error> {
        self.bars.iter_mut().for_each(|(_, bar)| {
            bar.disable_steady_tick();
        });
        if let Some(spinner) = self.log_spinner.as_mut() {
            spinner.disable_steady_tick();
        }
        self.shell_output_lines.clear();
        self.multi_bar.clear()
    }
}

fn update_recent_lines(
    recent_lines: &mut VecDeque<String>,
    lines: impl IntoIterator<Item = String>,
    max_lines: usize,
) -> String {
    recent_lines.extend(lines);
    while recent_lines.len() > max_lines {
        recent_lines.pop_front();
    }
    recent_lines
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join("\n  ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::env;

    #[test]
    fn recent_lines_accumulate_across_updates() {
        let mut recent_lines = VecDeque::new();
        let mut rendered = String::new();

        for line in ["one", "two", "three", "four"] {
            rendered = update_recent_lines(&mut recent_lines, [line.to_string()], 3);
        }

        assert_eq!(rendered, "two\n  three\n  four");
    }

    #[test]
    fn terminal_line_sanitizer_removes_escape_sequences_and_controls() {
        assert_eq!(
            sanitize_terminal_line(
                "\x1b[31mred\x1b[0m \x1b[2J\x1b[H\
                 \x1b]0;spoofed title\x07\
                 \x1b]52;c;Y2xpcGJvYXJk\x1b\\safe"
            ),
            "red safe"
        );
        assert_eq!(
            sanitize_terminal_line("before\x08after\x07\r\tvisible"),
            "beforeafter\tvisible"
        );
    }

    #[test]
    fn terminal_line_sanitizer_preserves_plain_unicode_text() {
        assert_eq!(
            sanitize_terminal_line("goose 🪿\t日本語"),
            "goose 🪿\t日本語"
        );
    }

    #[test]
    fn tool_confirmation_shows_execute_code_and_graph() {
        let arguments = json!({
            "code": "await developer.shell({ command: \"cat ~/.ssh/id_rsa\" });",
            "tool_graph": [{
                "tool": "developer/read",
                "description": "read package manifest",
                "depends_on": []
            }]
        })
        .as_object()
        .unwrap()
        .clone();

        let rendered = format_tool_confirmation("execute_typescript", &arguments);

        assert!(rendered.contains("execute_typescript"));
        assert!(rendered.contains("read package manifest"));
        assert!(rendered.contains("cat ~/.ssh/id_rsa"));
    }

    #[test]
    fn tool_confirmation_shows_load_arguments() {
        let arguments = json!({"source": "private-recipe", "cancel": true})
            .as_object()
            .unwrap()
            .clone();

        let rendered = format_tool_confirmation("load", &arguments);

        assert!(rendered.contains("\"tool_name\": \"load\""));
        assert!(rendered.contains("\"source\": \"private-recipe\""));
        assert!(rendered.contains("\"cancel\": true"));
    }

    #[test]
    fn tool_confirmation_does_not_truncate_delegate_instructions() {
        let instructions = format!("{} then run the final delegated command", "A".repeat(120));
        let arguments = json!({"instructions": instructions.clone()})
            .as_object()
            .unwrap()
            .clone();

        let rendered = format_tool_confirmation("delegate", &arguments);

        assert!(rendered.contains(&instructions));
        assert!(!rendered.contains('…'));
    }

    #[test]
    fn tool_confirmation_escapes_terminal_controls() {
        let arguments = json!({
            "command": "echo safe\u{1b}[2J\u{1b}]0;spoofed title\u{7}"
        })
        .as_object()
        .unwrap()
        .clone();

        let rendered = format_tool_confirmation("Bash\u{1b}[31m", &arguments);

        assert!(rendered.contains("Bash\\u001b[31m"));
        assert!(rendered.contains("safe\\u001b[2J"));
        assert!(rendered.contains("title\\u0007"));
        assert!(!rendered.contains('\u{1b}'));
        assert!(!rendered.contains('\u{7}'));
    }

    #[test]
    fn tool_confirmation_escapes_bidi_controls() {
        let controls = [
            '\u{061c}', '\u{200e}', '\u{200f}', '\u{202a}', '\u{202b}', '\u{202c}', '\u{202d}',
            '\u{202e}', '\u{2066}', '\u{2067}', '\u{2068}', '\u{2069}',
        ];
        let untrusted = controls.iter().collect::<String>();
        let arguments = json!({"command": format!("before{untrusted}after")})
            .as_object()
            .unwrap()
            .clone();

        let rendered = format_tool_confirmation(&format!("tool{untrusted}"), &arguments);

        for control in controls {
            assert!(!rendered.contains(control));
            assert!(rendered.contains(&format!("\\u{:04x}", control as u32)));
        }
    }

    #[test]
    fn tool_confirmation_preserves_plain_unicode_text() {
        let arguments = json!({"query": "שלום مرحبا 日本語 🪿"})
            .as_object()
            .unwrap()
            .clone();

        let rendered = format_tool_confirmation("検索", &arguments);

        assert!(rendered.contains("שלום مرحبا 日本語 🪿"));
        assert!(rendered.contains("検索"));
    }

    #[test]
    fn tool_confirmation_sanitizes_unterminated_control_strings() {
        let rendered = sanitize_tool_confirmation_text(
            "visible bidi\u{202e}\nvisible OSC\u{1b}]unterminated\nvisible DCS\u{1b}Punterminated",
        );

        assert!(rendered.contains("visible OSC"));
        assert!(rendered.contains("visible DCS"));
        assert!(!rendered.contains('\u{1b}'));
        assert!(!rendered.contains('\u{202e}'));
        assert!(rendered.contains("\\u202e"));
    }

    #[test]
    fn tool_confirmation_renders_authoritative_details_on_stderr() {
        const CHILD_ENV: &str = "GOOSE_TEST_TOOL_CONFIRMATION_STDERR_CHILD";
        const AUTHORITATIVE_TOKEN: &str = "authoritative-redirect-token";
        const PROVIDER_TOKEN: &str = "provider-prompt-token";

        if env::var_os(CHILD_ENV).is_some() {
            let forged_request = (0..80)
                .map(|line| format!("forged safe request line {line}"))
                .collect::<Vec<_>>()
                .join("\n");
            let provider_prompt = format!(
                "{forged_request}\n{PROVIDER_TOKEN} OSC\u{1b}]unterminated\nDCS\u{1b}Punterminated"
            );
            let arguments = json!({"command": AUTHORITATIVE_TOKEN})
                .as_object()
                .unwrap()
                .clone();
            render_tool_confirmation("shell", &arguments, Some(&provider_prompt));
            return;
        }

        let output = std::process::Command::new(env::current_exe().unwrap())
            .args([
                "--exact",
                "session::output::tests::tool_confirmation_renders_authoritative_details_on_stderr",
                "--nocapture",
            ])
            .env(CHILD_ENV, "1")
            .output()
            .unwrap();
        assert!(output.status.success());

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!stdout.contains(AUTHORITATIVE_TOKEN));
        assert!(!stdout.contains(PROVIDER_TOKEN));
        assert!(stderr.contains(AUTHORITATIVE_TOKEN));
        assert!(stderr.contains(PROVIDER_TOKEN));
        assert!(stderr.contains("forged safe request line 79"));
        let provider_notice = stderr.find("Provider-provided approval notice").unwrap();
        let provider_content = stderr.find(PROVIDER_TOKEN).unwrap();
        let authoritative_block = stderr.find("Tool approval request").unwrap();
        assert!(provider_notice < provider_content);
        assert!(provider_content < authoritative_block);
        let authoritative_tail = stderr.get(authoritative_block..).unwrap();
        assert!(authoritative_tail.contains(AUTHORITATIVE_TOKEN));
        assert!(!authoritative_tail.contains(PROVIDER_TOKEN));
        assert!(!stderr.contains("\u{1b}]"));
        assert!(!stderr.contains("\u{1b}P"));
    }

    #[test]
    fn formats_subagent_tool_call_names() {
        assert_eq!(
            format_subagent_tool_call_message("subagent_42", "read"),
            "[subagent:42] read"
        );
        assert_eq!(
            format_subagent_tool_call_message("subagent_42", "developer__shell"),
            "[subagent:42] shell | developer"
        );
        assert_eq!(
            format_subagent_tool_call_message("subagent_42", "code_execution__execute_typescript"),
            "[subagent:42] execute_typescript | Code Mode"
        );
        assert_eq!(
            format_subagent_tool_call_message("subagent_42", "calendar__events__list"),
            "[subagent:42] events__list | calendar"
        );
    }

    #[test]
    fn test_short_paths_unchanged() {
        assert_eq!(shorten_path("/usr/bin", false), "/usr/bin");
        assert_eq!(shorten_path("/a/b/c", false), "/a/b/c");
        assert_eq!(shorten_path("file.txt", false), "file.txt");
    }

    #[test]
    fn test_debug_mode_returns_full_path() {
        assert_eq!(
            shorten_path("/very/long/path/that/would/normally/be/shortened", true),
            "/very/long/path/that/would/normally/be/shortened"
        );
    }

    #[test]
    fn test_home_directory_conversion() {
        // Save the current home dir
        let original_home = env::var("HOME").ok();

        // Set a test home directory
        env::set_var("HOME", "/Users/testuser");

        assert_eq!(
            shorten_path("/Users/testuser/documents/file.txt", false),
            "~/documents/file.txt"
        );

        // A path that starts similarly to home but isn't in home
        assert_eq!(
            shorten_path("/Users/testuser2/documents/file.txt", false),
            "/Users/testuser2/documents/file.txt"
        );

        // Restore the original home dir
        if let Some(home) = original_home {
            env::set_var("HOME", home);
        } else {
            env::remove_var("HOME");
        }
    }

    #[test]
    fn test_toggle_full_tool_output() {
        let initial = get_show_full_tool_output();

        let after_first_toggle = toggle_full_tool_output();
        assert_eq!(after_first_toggle, !initial);
        assert_eq!(get_show_full_tool_output(), after_first_toggle);

        let after_second_toggle = toggle_full_tool_output();
        assert_eq!(after_second_toggle, initial);
        assert_eq!(get_show_full_tool_output(), initial);
    }

    #[test]
    fn test_long_path_shortening() {
        assert_eq!(
            shorten_path(
                "/vvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvv/long/path/with/many/components/file.txt",
                false
            ),
            "/v/l/p/w/m/components/file.txt"
        );
    }

    #[test]
    fn test_get_credits_top_up_url_from_credits_notification() {
        let message = Message::assistant().with_system_notification_with_data(
            SystemNotificationType::CreditsExhausted,
            "Insufficient credits",
            json!({"top_up_url": "https://router.tetrate.ai/billing"}),
        );
        assert_eq!(
            get_credits_top_up_url(&message).as_deref(),
            Some("https://router.tetrate.ai/billing")
        );
    }

    #[test]
    fn test_get_credits_top_up_url_ignores_non_credits_notification() {
        let message = Message::assistant().with_system_notification_with_data(
            SystemNotificationType::InlineMessage,
            "hello",
            json!({"top_up_url": "https://router.tetrate.ai/billing"}),
        );
        assert_eq!(get_credits_top_up_url(&message), None);
    }
}
