use std::path::Path;
use std::sync::LazyLock;

use anyhow::Result;
use goose_providers::conversation::{message::Message, Conversation};
use regex::Regex;

use crate::{providers::base::Provider, utils::safe_truncate};

pub static MSG_COUNT_FOR_SESSION_NAME_GENERATION: usize = 3;

fn strip_xml_tags(text: &str) -> String {
    static BLOCK_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?s)<([a-zA-Z][a-zA-Z0-9_]*)[^>]*>.*?</[a-zA-Z][a-zA-Z0-9_]*>").unwrap()
    });
    static TAG_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"</?[a-zA-Z][a-zA-Z0-9_]*[^>]*>").unwrap());
    let pass1 = BLOCK_RE.replace_all(text, "");
    TAG_RE.replace_all(&pass1, "").into_owned()
}

fn extract_short_title(text: &str) -> String {
    let word_count = text.split_whitespace().count();
    if word_count <= 8 {
        return text.to_string();
    }

    {
        let mut results = Vec::new();
        let mut quote_char: Option<char> = None;
        let mut current = String::new();
        let mut prev_char: Option<char> = None;

        for ch in text.chars() {
            match quote_char {
                None => {
                    if matches!(ch, '"' | '\'' | '`') {
                        let after_alnum = prev_char.map(|p| p.is_alphanumeric()).unwrap_or(false);
                        if !after_alnum {
                            quote_char = Some(ch);
                            current.clear();
                        }
                    }
                }
                Some(q) => {
                    if ch == q {
                        let trimmed = current.trim().to_string();
                        let wc = trimmed.split_whitespace().count();
                        if (2..=8).contains(&wc) {
                            results.push(trimmed);
                        }
                        quote_char = None;
                        current.clear();
                    } else {
                        current.push(ch);
                    }
                }
            }
            prev_char = Some(ch);
        }

        if let Some(title) = results.last() {
            return title.clone();
        }
    }

    if let Some(last) = text.lines().rev().find(|l| !l.trim().is_empty()) {
        return last.trim().to_string();
    }

    text.to_string()
}

fn get_initial_user_messages(messages: &Conversation) -> Vec<String> {
    messages
        .iter()
        .filter(|m| m.role == rmcp::model::Role::User && m.is_user_visible())
        .take(MSG_COUNT_FOR_SESSION_NAME_GENERATION)
        .map(|m| {
            m.content
                .iter()
                .filter_map(|c| c.filter_for_audience(rmcp::model::Role::User))
                .filter_map(|c| c.as_text().map(|s| s.to_string()))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .collect()
}

pub(crate) async fn generate_session_name(
    provider: &dyn Provider,
    model_config: &goose_providers::model::ModelConfig,
    session_id: &str,
    messages: &Conversation,
    working_dir: Option<&Path>,
) -> Result<String> {
    let context = get_initial_user_messages(messages);
    let system = crate::prompt_template::render_template(
        "session_name.md",
        &std::collections::HashMap::<String, String>::new(),
    )?;

    use crate::providers::cli_common::{
        SESSION_NAME_BEGIN_MARKER, SESSION_NAME_END_MARKER, SESSION_NAME_SUFFIX,
    };

    let hint = working_dir
        .and_then(Path::file_name)
        .and_then(|folder| folder.to_str())
        .map(|folder| format!("working folder: {folder}"))
        .unwrap_or_default();
    let hints_section = if hint.is_empty() {
        String::new()
    } else {
        format!(
            "---BEGIN HINTS (optional signals like the working folder; use them only when they match the subject of the messages)---\n{hint}\n---END HINTS---\n\n"
        )
    };
    let user_text = format!(
        "{}{}\n{}\n{}\n\n{}",
        hints_section,
        SESSION_NAME_BEGIN_MARKER,
        context.join("\n"),
        SESSION_NAME_END_MARKER,
        SESSION_NAME_SUFFIX,
    );
    let message = Message::user().with_text(&user_text);
    let result = if provider.uses_local_session_naming() {
        crate::providers::cli_common::generate_simple_session_description(
            provider.get_name(),
            &[message],
        )?
    } else {
        crate::model_config::complete_one_shot(
            provider,
            model_config,
            session_id,
            &system,
            &[message],
            &[],
        )
        .await?
    };

    let raw: String = result
        .0
        .content
        .iter()
        .filter_map(|c| c.as_text())
        .collect();
    let description = strip_xml_tags(&raw)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");

    Ok(safe_truncate(&extract_short_title(&description), 100))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_xml_tags() {
        assert_eq!(strip_xml_tags("<think>reasoning</think>answer"), "answer");
        assert_eq!(strip_xml_tags("before<t>mid</t>after"), "beforeafter");
        assert_eq!(strip_xml_tags("<a>x</a><b>y</b>z"), "z");
        assert_eq!(strip_xml_tags("no tags here"), "no tags here");
        assert_eq!(strip_xml_tags("a < b > c"), "a < b > c");
        assert_eq!(strip_xml_tags("<think>über</think>ok"), "ok");
        assert_eq!(strip_xml_tags("<think>日本語</think>hello"), "hello");
        assert_eq!(strip_xml_tags(""), "");
        assert_eq!(strip_xml_tags("<>stuff</>"), "<>stuff</>");
        // attributes
        assert_eq!(
            strip_xml_tags(r#"<think class="deep">reasoning</think>answer"#),
            "answer"
        );
        // self-closing tags
        assert_eq!(strip_xml_tags("<br/>self closing"), "self closing");
        // orphan closing tags
        assert_eq!(strip_xml_tags("orphan </think> tag"), "orphan  tag");
        // multiline content
        assert_eq!(
            strip_xml_tags("<think>\nline1\nline2\n</think>result"),
            "result"
        );
    }

    #[test]
    fn test_extract_short_title() {
        assert_eq!(extract_short_title("List files"), "List files");
        assert_eq!(
            extract_short_title(
                r#"blah blah blah blah blah blah blah blah blah "List files in folder""#
            ),
            "List files in folder"
        );
        assert_eq!(
            extract_short_title(
                "blah blah blah blah blah blah blah blah blah `View current files`"
            ),
            "View current files"
        );
        assert_eq!(
            extract_short_title(
                r#"stuff stuff stuff stuff stuff stuff stuff stuff "Abc title" "Zzz title""#
            ),
            "Zzz title"
        );
        assert_eq!(
            extract_short_title(
                "long long long long long long long long long\nList files in folder"
            ),
            "List files in folder"
        );
        assert_eq!(
            extract_short_title(
                r#"lots of words here and there and more and more "single" final line here"#
            ),
            "lots of words here and there and more and more \"single\" final line here"
        );
        assert_eq!(extract_short_title("Hello world"), "Hello world");
        assert_eq!(
            extract_short_title(
                r#"1. Analyze the request. 2. The user's message says list files. 3. "List current folder files" fits perfectly. Result: List current folder files"#
            ),
            "List current folder files"
        );
        assert_eq!(
            extract_short_title(
                r#"the user's phrasing is about listing files and the user's intent is clear. "List folder files" is best"#
            ),
            "List folder files"
        );
        assert_eq!(
            extract_short_title(
                "lots of reasoning here about what to call it\nList current folder files"
            ),
            "List current folder files"
        );
    }
}
