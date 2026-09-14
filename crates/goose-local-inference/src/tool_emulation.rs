//! Shared text-based tool call emulation for local inference backends.
//!
//! Models that do not have native tool-calling support are prompted to emit shell commands
//! as `$ command` on a new line and code blocks as ```execute_typescript fenced blocks.
//! The parser converts those patterns into Goose tool-call messages.

use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag};

#[cfg(feature = "mlx")]
use goose_provider_types::conversation::message::{Message, MessageContent};
#[cfg(feature = "mlx")]
use rmcp::model::{CallToolRequestParams, Tool};
#[cfg(feature = "mlx")]
use serde_json::json;
#[cfg(feature = "mlx")]
use std::borrow::Cow;
#[cfg(feature = "mlx")]
use uuid::Uuid;

#[cfg(feature = "mlx")]
pub(crate) const SHELL_TOOL: &str = "developer__shell";
#[cfg(feature = "mlx")]
pub(crate) const CODE_EXECUTION_TOOL: &str = "code_execution__execute_typescript";

#[cfg(feature = "mlx")]
pub(crate) fn load_tiny_model_prompt() -> String {
    use std::env;

    let os = if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "unknown"
    };

    let working_directory = env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    let shell = env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());

    let context = json!({
        "os": os,
        "working_directory": working_directory,
        "shell": shell,
    });

    crate::prompt_template::render_template("tiny_model_system.md", &context).unwrap_or_else(|e| {
        tracing::warn!("Failed to load tiny_model_system.md: {:?}", e);
        "You are Goose, an AI assistant. You can execute shell commands by starting lines with $."
            .to_string()
    })
}

#[cfg(feature = "mlx")]
pub(crate) fn build_emulator_tool_description(tools: &[Tool], code_mode_enabled: bool) -> String {
    let mut tool_desc = String::new();

    if code_mode_enabled {
        tool_desc.push_str("\n\n# Running Code\n\n");
        tool_desc.push_str(
            "You can call tools by writing code in a ```execute_typescript block. \
             The code runs immediately — do not explain it, just run it.\n\n",
        );
        tool_desc.push_str("Example — counting files in /tmp:\n\n");
        tool_desc.push_str("```execute_typescript\nasync function run() {\n");
        tool_desc.push_str(
            "  const result = await Developer.shell({ command: \"ls -1 /tmp | wc -l\" });\n",
        );
        tool_desc.push_str("  return result;\n}\n```\n\n");
        tool_desc.push_str("Rules:\n");
        tool_desc.push_str("- Code MUST define async function run() and return a result\n");
        tool_desc.push_str("- All function calls are async — use await\n");
        tool_desc.push_str(
            "- Use ```execute_typescript for tool calls, $ for simple shell one-liners\n\n",
        );
        tool_desc.push_str("Available functions:\n\n");

        for tool in tools {
            if tool.name.starts_with("code_execution__") {
                continue;
            }
            let parts: Vec<&str> = tool.name.splitn(2, "__").collect();
            if parts.len() == 2 {
                let namespace = {
                    let mut c = parts[0].chars();
                    match c.next() {
                        None => String::new(),
                        Some(first) => first.to_uppercase().chain(c).collect::<String>(),
                    }
                };
                let camel_name: String = parts[1]
                    .split('_')
                    .enumerate()
                    .map(|(i, part)| {
                        if i == 0 {
                            part.to_string()
                        } else {
                            let mut c = part.chars();
                            match c.next() {
                                None => String::new(),
                                Some(first) => first.to_uppercase().chain(c).collect(),
                            }
                        }
                    })
                    .collect();
                let desc = tool.description.as_ref().map(|d| d.as_ref()).unwrap_or("");
                tool_desc.push_str(&format!("- {namespace}.{camel_name}(): {desc}\n"));
            }
        }
    } else {
        tool_desc.push_str("\n\n# Tools\n\nYou have access to the following tools:\n\n");
        for tool in tools {
            let desc = tool
                .description
                .as_ref()
                .map(|d| d.as_ref())
                .unwrap_or("No description");
            tool_desc.push_str(&format!("- {}: {}\n", tool.name, desc));
        }
    }

    tool_desc
}

pub(crate) enum EmulatorAction {
    Text(String),
    ShellCommand(String),
    ExecuteCode(String),
}

// pulldown-cmark parses complete documents, so fail closed once reparsing would become expensive.
const MAX_MARKDOWN_CONTEXT_BYTES: usize = 256 * 1024;
const MAX_MARKDOWN_PARSE_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy)]
enum ParserState {
    Normal,
    InExecuteBlock { fence_len: usize },
}

pub(crate) struct StreamingEmulatorParser {
    buffer: String,
    document: String,
    markdown_context_available: bool,
    markdown_parse_bytes_remaining: usize,
    state: ParserState,
    code_mode_enabled: bool,
}

fn execute_fence_len(line: &str) -> Option<usize> {
    let line = line.strip_suffix('\r').unwrap_or(line);
    let indent = line.bytes().take_while(|byte| *byte == b' ').count();
    if indent > 3 {
        return None;
    }

    let rest = line.get(indent..)?;
    let fence_len = rest.bytes().take_while(|byte| *byte == b'`').count();
    if fence_len < 3 {
        return None;
    }

    rest.get(fence_len..)?
        .trim_matches([' ', '\t'])
        .eq("execute_typescript")
        .then_some(fence_len)
}

fn is_closing_fence(line: &str, minimum_len: usize) -> bool {
    let line = line.strip_suffix('\r').unwrap_or(line);
    let indent = line.bytes().take_while(|byte| *byte == b' ').count();
    if indent > 3 {
        return false;
    }

    let Some(rest) = line.get(indent..) else {
        return false;
    };
    let fence_len = rest.bytes().take_while(|byte| *byte == b'`').count();
    fence_len >= minimum_len
        && rest
            .get(fence_len..)
            .is_some_and(|suffix| suffix.bytes().all(|byte| matches!(byte, b' ' | b'\t')))
}

fn is_top_level_execute_fence(markdown: &str, line_start: usize) -> bool {
    let mut depth = 0;
    for (event, range) in Parser::new(markdown).into_offset_iter() {
        match event {
            Event::Start(tag) => {
                if depth == 0
                    && range.start == line_start
                    && matches!(
                        tag,
                        Tag::CodeBlock(CodeBlockKind::Fenced(ref info))
                            if info.as_ref() == "execute_typescript"
                    )
                {
                    return true;
                }
                depth += 1;
            }
            Event::End(_) => depth -= 1,
            _ => {}
        }
    }
    false
}

fn is_top_level_paragraph_line(markdown: &str, line_start: usize) -> bool {
    let mut depth = 0;
    for (event, range) in Parser::new(markdown).into_offset_iter() {
        match event {
            Event::Start(tag) => {
                if depth == 0
                    && matches!(tag, Tag::Paragraph)
                    && range.start <= line_start
                    && line_start < range.end
                {
                    return true;
                }
                depth += 1;
            }
            Event::End(_) => depth -= 1,
            _ => {}
        }
    }
    false
}

fn closing_fence_range(
    input: &str,
    minimum_len: usize,
    allow_end_of_stream: bool,
) -> Option<(usize, usize)> {
    let mut line_start = 0;
    loop {
        let remaining = input
            .get(line_start..)
            .expect("line start must be a character boundary");
        let newline_offset = remaining.find('\n');
        if newline_offset.is_none() && !allow_end_of_stream {
            return None;
        }
        let line_end = newline_offset
            .map(|offset| line_start + offset)
            .unwrap_or(input.len());
        if is_closing_fence(
            input
                .get(line_start..line_end)
                .expect("line range must be on character boundaries"),
            minimum_len,
        ) {
            let consumed = if line_end < input.len() {
                line_end + 1
            } else {
                line_end
            };
            return Some((line_start, consumed));
        }
        if line_end == input.len() {
            return None;
        }
        line_start = line_end + 1;
    }
}

impl StreamingEmulatorParser {
    pub(crate) fn new(code_mode_enabled: bool) -> Self {
        Self {
            buffer: String::new(),
            document: String::new(),
            markdown_context_available: true,
            markdown_parse_bytes_remaining: MAX_MARKDOWN_PARSE_BYTES,
            state: ParserState::Normal,
            code_mode_enabled,
        }
    }

    fn append_markdown_context(&mut self, chunk: &str) {
        if !self.markdown_context_available {
            return;
        }

        if self.document.len().saturating_add(chunk.len()) > MAX_MARKDOWN_CONTEXT_BYTES {
            self.disable_markdown_emulation();
        } else {
            self.document.push_str(chunk);
        }
    }

    fn consume_markdown_parse_budget(&mut self, bytes: usize) -> bool {
        if bytes > self.markdown_parse_bytes_remaining {
            self.disable_markdown_emulation();
            false
        } else {
            self.markdown_parse_bytes_remaining -= bytes;
            true
        }
    }

    fn markdown_matches(
        &mut self,
        range: Option<(usize, usize)>,
        predicate: fn(&str, usize) -> bool,
    ) -> bool {
        let Some((line_start, markdown_end)) = range else {
            return false;
        };
        if !self.consume_markdown_parse_budget(markdown_end) {
            return false;
        }

        let markdown = self
            .document
            .get(..markdown_end)
            .expect("markdown end must be a character boundary");
        predicate(markdown, line_start)
    }

    fn disable_markdown_emulation(&mut self) {
        self.document.clear();
        self.markdown_context_available = false;
        self.markdown_parse_bytes_remaining = 0;
    }

    pub(crate) fn process_chunk(&mut self, chunk: &str) -> Vec<EmulatorAction> {
        self.buffer.push_str(chunk);
        self.append_markdown_context(chunk);
        let mut results = Vec::new();

        loop {
            match self.state {
                ParserState::InExecuteBlock { fence_len } => {
                    let Some((closing_start, consumed)) =
                        closing_fence_range(&self.buffer, fence_len, false)
                    else {
                        break;
                    };
                    let code_end = closing_start
                        .checked_sub(1)
                        .filter(|index| self.buffer.as_bytes()[*index] == b'\n')
                        .unwrap_or(closing_start);
                    let code = self
                        .buffer
                        .get(..code_end)
                        .expect("code boundary must be a character boundary");
                    let code = code.strip_suffix('\r').unwrap_or(code).to_string();
                    self.buffer.replace_range(..consumed, "");
                    self.state = ParserState::Normal;
                    if !code.trim().is_empty() {
                        results.push(EmulatorAction::ExecuteCode(code));
                    }
                }
                ParserState::Normal => {
                    let Some(line_end) = self.buffer.find('\n') else {
                        break;
                    };
                    let markdown_range = self.markdown_context_available.then(|| {
                        let line_start = self.document.len() - self.buffer.len();
                        (line_start, line_start + line_end + 1)
                    });
                    let line = self
                        .buffer
                        .get(..line_end)
                        .expect("line end must be a character boundary")
                        .to_string();
                    let line_with_newline = self
                        .buffer
                        .get(..=line_end)
                        .expect("newline must be a character boundary")
                        .to_string();
                    self.buffer.replace_range(..=line_end, "");

                    if self.code_mode_enabled {
                        if let Some(fence_len) = execute_fence_len(&line) {
                            if self.markdown_matches(markdown_range, is_top_level_execute_fence) {
                                self.state = ParserState::InExecuteBlock { fence_len };
                                continue;
                            }
                        }
                    }

                    let line_without_cr = line.strip_suffix('\r').unwrap_or(&line);
                    if let Some(command) = line_without_cr.strip_prefix('$') {
                        if self.markdown_matches(markdown_range, is_top_level_paragraph_line) {
                            let command = command.trim();
                            if !command.is_empty() {
                                results.push(EmulatorAction::ShellCommand(command.to_string()));
                            }
                            continue;
                        }
                    }

                    results.push(EmulatorAction::Text(line_with_newline));
                }
            }
        }

        results
    }

    pub(crate) fn flush(&mut self) -> Vec<EmulatorAction> {
        let mut results = self.process_chunk("");

        match self.state {
            ParserState::InExecuteBlock { fence_len } => {
                let code_end = closing_fence_range(&self.buffer, fence_len, true)
                    .map(|(closing_start, _)| {
                        closing_start
                            .checked_sub(1)
                            .filter(|index| self.buffer.as_bytes()[*index] == b'\n')
                            .unwrap_or(closing_start)
                    })
                    .unwrap_or(self.buffer.len());
                let code = self
                    .buffer
                    .get(..code_end)
                    .expect("code boundary must be a character boundary");
                let code = code.strip_suffix('\r').unwrap_or(code).trim();
                if !code.is_empty() {
                    results.push(EmulatorAction::ExecuteCode(code.to_string()));
                }
            }
            ParserState::Normal if !self.buffer.is_empty() => {
                let line = self
                    .buffer
                    .strip_suffix('\r')
                    .unwrap_or(&self.buffer)
                    .to_string();
                let markdown_range = self.markdown_context_available.then(|| {
                    let line_start = self.document.len() - self.buffer.len();
                    (line_start, self.document.len())
                });

                if self.code_mode_enabled
                    && execute_fence_len(&line).is_some()
                    && self.markdown_matches(markdown_range, is_top_level_execute_fence)
                {
                } else if let Some(command) = line.strip_prefix('$') {
                    if self.markdown_matches(markdown_range, is_top_level_paragraph_line) {
                        let command = command.trim();
                        if !command.is_empty() {
                            results.push(EmulatorAction::ShellCommand(command.to_string()));
                        }
                    } else {
                        results.push(EmulatorAction::Text(self.buffer.clone()));
                    }
                } else {
                    results.push(EmulatorAction::Text(self.buffer.clone()));
                }
            }
            ParserState::Normal => {}
        }

        self.buffer.clear();
        self.document.clear();
        self.markdown_context_available = true;
        self.markdown_parse_bytes_remaining = MAX_MARKDOWN_PARSE_BYTES;
        self.state = ParserState::Normal;
        results
    }
}

#[cfg(feature = "mlx")]
pub(crate) fn message_for_emulator_action(
    action: &EmulatorAction,
    message_id: &str,
) -> (Message, bool) {
    match action {
        EmulatorAction::Text(text) => {
            let mut message = Message::assistant().with_text(text);
            message.id = Some(message_id.to_string());
            (message, false)
        }
        EmulatorAction::ShellCommand(command) => {
            let tool_id = Uuid::new_v4().to_string();
            let mut args = serde_json::Map::new();
            args.insert("command".to_string(), json!(command));
            let tool_call =
                CallToolRequestParams::new(Cow::Borrowed(SHELL_TOOL)).with_arguments(args);
            let mut message = Message::assistant();
            message
                .content
                .push(MessageContent::tool_request(tool_id, Ok(tool_call)));
            message.id = Some(message_id.to_string());
            (message, true)
        }
        EmulatorAction::ExecuteCode(code) => {
            let tool_id = Uuid::new_v4().to_string();
            let wrapped = if code.contains("async function run()") {
                code.clone()
            } else {
                format!("async function run() {{\n{}\n}}", code)
            };
            let mut args = serde_json::Map::new();
            args.insert("code".to_string(), json!(wrapped));
            let tool_call =
                CallToolRequestParams::new(Cow::Borrowed(CODE_EXECUTION_TOOL)).with_arguments(args);
            let mut message = Message::assistant();
            message
                .content
                .push(MessageContent::tool_request(tool_id, Ok(tool_call)));
            message.id = Some(message_id.to_string());
            (message, true)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_chunks(chunks: &[&str], code_mode: bool) -> Vec<EmulatorAction> {
        let mut parser = StreamingEmulatorParser::new(code_mode);
        let mut actions = Vec::new();
        for chunk in chunks {
            actions.extend(parser.process_chunk(chunk));
        }
        actions.extend(parser.flush());
        actions
    }

    fn parse_all(input: &str, code_mode: bool) -> Vec<EmulatorAction> {
        parse_chunks(&[input], code_mode)
    }

    fn text(actions: &[EmulatorAction]) -> String {
        actions
            .iter()
            .filter_map(|action| match action {
                EmulatorAction::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    fn shell_commands(actions: &[EmulatorAction]) -> Vec<&str> {
        actions
            .iter()
            .filter_map(|action| match action {
                EmulatorAction::ShellCommand(command) => Some(command.as_str()),
                _ => None,
            })
            .collect()
    }

    fn execute_blocks(actions: &[EmulatorAction]) -> Vec<&str> {
        actions
            .iter()
            .filter_map(|action| match action {
                EmulatorAction::ExecuteCode(code) => Some(code.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn plain_text_is_preserved() {
        let input = "Hello, world!\n";
        let actions = parse_all(input, false);

        assert_eq!(text(&actions), input);
        assert!(shell_commands(&actions).is_empty());
        assert!(execute_blocks(&actions).is_empty());
    }

    #[test]
    fn top_level_shell_commands_are_detected() {
        let actions = parse_chunks(&["Let me check:\n", "$ who", "ami\n$ pwd"], false);

        assert_eq!(shell_commands(&actions), ["whoami", "pwd"]);
        assert_eq!(text(&actions), "Let me check:\n");
    }

    #[test]
    fn dollar_sign_mid_sentence_is_text() {
        let input = "It costs $50 per month";
        let actions = parse_all(input, false);

        assert_eq!(text(&actions), input);
        assert!(shell_commands(&actions).is_empty());
    }

    #[test]
    fn markdown_contained_shell_commands_remain_text() {
        let cases = [
            "````markdown\n$ inert\n````\n",
            "> $ inert\n",
            "- $ inert\n",
            "<div>\n$ inert\n\n",
        ];

        for input in cases {
            let actions = parse_all(input, true);
            assert_eq!(text(&actions), input, "input: {input:?}");
            assert!(shell_commands(&actions).is_empty(), "input: {input:?}");
        }
    }

    #[test]
    fn top_level_execute_block_after_text_is_detected() {
        let input = "Here's the code:\n```execute_typescript\nconsole.log('hi');\n```\n";
        let actions = parse_all(input, true);

        assert_eq!(text(&actions), "Here's the code:\n");
        assert_eq!(execute_blocks(&actions), ["console.log('hi');"]);
    }

    #[test]
    fn execute_fence_can_be_split_across_chunks() {
        let input = "```execute_typescript\nlet x = 1;\n```\n";
        let chunks: Vec<String> = input.chars().map(|ch| ch.to_string()).collect();
        let chunk_refs: Vec<&str> = chunks.iter().map(String::as_str).collect();
        let actions = parse_chunks(&chunk_refs, true);

        assert_eq!(execute_blocks(&actions), ["let x = 1;"]);
    }

    #[test]
    fn execute_blocks_are_disabled_without_code_mode() {
        let input = "```execute_typescript\nlet x = 1;\n```\n";
        let actions = parse_all(input, false);

        assert_eq!(text(&actions), input);
        assert!(execute_blocks(&actions).is_empty());
    }

    #[test]
    fn nested_execute_fences_remain_text() {
        let cases = [
            "````markdown\n```execute_typescript\ninert();\n```\n````\n",
            "~~~markdown\n```execute_typescript\ninert();\n```\n~~~\n",
            "> ```execute_typescript\n> inert();\n> ```\n",
            "- ```execute_typescript\n  inert();\n  ```\n",
            "<div>\n```execute_typescript\ninert();\n```\n\n",
        ];

        for input in cases {
            let actions = parse_all(input, true);
            assert_eq!(text(&actions), input, "input: {input:?}");
            assert!(execute_blocks(&actions).is_empty(), "input: {input:?}");
        }
    }

    #[test]
    fn invalid_fence_info_does_not_hide_following_execute() {
        let input = "```execute_typescript```\n```execute_typescript\nsafe();\n```\n";
        let actions = parse_all(input, true);

        assert_eq!(execute_blocks(&actions), ["safe();"]);
        assert!(text(&actions).contains("```execute_typescript```"));
    }

    #[test]
    fn longer_execute_fence_requires_matching_close() {
        let input = "````execute_typescript\nlet before = 1;\n```\nlet after = 2;\n````\n";
        let actions = parse_all(input, true);

        assert_eq!(
            execute_blocks(&actions),
            ["let before = 1;\n```\nlet after = 2;"]
        );
    }

    #[test]
    fn closing_fence_waits_for_complete_line() {
        let mut parser = StreamingEmulatorParser::new(true);

        assert!(parser
            .process_chunk("```execute_typescript\nlet x = 1;\n```")
            .iter()
            .all(|action| !matches!(action, EmulatorAction::ExecuteCode(_))));
        assert!(parser
            .process_chunk("not-a-close")
            .iter()
            .all(|action| !matches!(action, EmulatorAction::ExecuteCode(_))));
        let actions = parser.process_chunk("\n```\n");

        assert_eq!(execute_blocks(&actions), ["let x = 1;\n```not-a-close"]);
    }

    #[test]
    fn unicode_whitespace_does_not_close_execute_fence() {
        let input = "```execute_typescript\nlet before = 1;\n```\u{a0}\nlet after = 2;\n```\n";
        let actions = parse_all(input, true);

        assert_eq!(
            execute_blocks(&actions),
            ["let before = 1;\n```\u{a0}\nlet after = 2;"]
        );
    }

    #[test]
    fn markdown_parse_work_is_bounded() {
        let mut input = String::from("````markdown\n");
        for _ in 0..1000 {
            input.push_str("$ inert\n");
        }
        input.push_str("````\n$ still inert\n");

        let actions = parse_all(&input, true);

        assert_eq!(text(&actions), input);
        assert!(shell_commands(&actions).is_empty());
        assert!(execute_blocks(&actions).is_empty());
    }

    #[test]
    fn eof_flushes_closed_and_unclosed_execute_blocks() {
        for (input, expected) in [
            ("```execute_typescript\nsafe();\n```", "safe();"),
            ("```execute_typescript\nsafe();", "safe();"),
        ] {
            let actions = parse_all(input, true);
            assert_eq!(execute_blocks(&actions), [expected]);
        }
    }

    #[cfg(feature = "mlx")]
    #[test]
    fn tool_description_uses_execute_typescript_fence() {
        let description = build_emulator_tool_description(&[], true);

        assert!(description.contains("```execute_typescript"));
    }
}
