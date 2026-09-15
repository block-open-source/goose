use crate::templates::{self, COMPACTION_SUMMARY_TEMPLATE};
use goose_provider_types::json::safely_parse_json;
use serde::{Deserialize, Serialize};

/// Structured output of the compaction LLM call.
///
/// Every list is ordered most-important-first so consumers (the render
/// template, experiments that truncate sections) can cut from the tail.
/// Fields deserialize leniently - omitted fields default to empty, and an
/// object or number where a string was asked for is stringified rather than
/// failing - because models routinely enrich the schema (e.g. `{"error": ..,
/// "fix": ..}` entries) and one such field must not discard a good summary.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StructuredSummary {
    #[serde(default, deserialize_with = "lenient_string_list")]
    pub user_intent: Vec<String>,
    #[serde(default, deserialize_with = "lenient_string_list")]
    pub technical_concepts: Vec<String>,
    #[serde(default, deserialize_with = "lenient_file_list")]
    pub files: Vec<FileActivity>,
    #[serde(default, deserialize_with = "lenient_string_list")]
    pub errors_and_fixes: Vec<String>,
    #[serde(default, deserialize_with = "lenient_string_list")]
    pub problem_solving: Vec<String>,
    #[serde(default, deserialize_with = "lenient_string_list")]
    pub user_messages: Vec<String>,
    #[serde(default, deserialize_with = "lenient_string_list")]
    pub pending_tasks: Vec<String>,
    #[serde(default, deserialize_with = "lenient_string_opt")]
    pub current_work: Option<String>,
    #[serde(default, deserialize_with = "lenient_string_opt")]
    pub next_step: Option<String>,
    /// Unknown top-level fields, kept so a user-customized compaction prompt
    /// that adds fields can still reach them from a customized render
    /// template. Not counted when deciding whether a summary is empty.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileActivity {
    #[serde(default, deserialize_with = "lenient_string")]
    pub path: String,
    #[serde(default, deserialize_with = "lenient_string")]
    pub summary: String,
    #[serde(default, deserialize_with = "lenient_string_opt")]
    pub key_code: Option<String>,
}

fn stringify_lenient(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => String::new(),
        serde_json::Value::Object(map) => map
            .iter()
            .map(|(k, v)| format!("{k}: {}", stringify_lenient(v)))
            .collect::<Vec<_>>()
            .join("; "),
        serde_json::Value::Array(items) => items
            .iter()
            .map(stringify_lenient)
            .collect::<Vec<_>>()
            .join("; "),
        other => other.to_string(),
    }
}

fn lenient_string_list<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(match value {
        serde_json::Value::Array(items) => items.iter().map(stringify_lenient).collect(),
        serde_json::Value::Null => Vec::new(),
        other => vec![stringify_lenient(&other)],
    })
}

fn lenient_string<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(stringify_lenient(&value))
}

fn lenient_string_opt<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(match value {
        serde_json::Value::Null => None,
        other => Some(stringify_lenient(&other)),
    })
}

/// `files` entries should be objects, but a model that over-applies the
/// "plain strings" rule may emit them as strings; render those as path-only
/// activities rather than discarding the whole summary.
fn lenient_file_list<'de, D>(deserializer: D) -> Result<Vec<FileActivity>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    let items = match value {
        serde_json::Value::Array(items) => items,
        serde_json::Value::Null => return Ok(Vec::new()),
        other => vec![other],
    };
    Ok(items
        .into_iter()
        .filter_map(|item| match item {
            serde_json::Value::Object(_) => serde_json::from_value(item).ok(),
            other => {
                let path = stringify_lenient(&other);
                (!path.trim().is_empty()).then_some(FileActivity {
                    path,
                    summary: String::new(),
                    key_code: None,
                })
            }
        })
        .collect())
}

impl StructuredSummary {
    /// Returns `None` when no usable JSON document is found so the caller can
    /// keep the raw response text - the lossless fallback.
    pub fn parse(response_text: &str) -> Option<Self> {
        json_candidates(response_text).into_iter().find_map(|c| {
            let value = safely_parse_json(c).ok()?;
            let mut summary: Self = serde_json::from_value(value).ok()?;
            summary.normalize();
            (!summary.is_empty()).then_some(summary)
        })
    }

    pub fn render(&self) -> Result<String, minijinja::Error> {
        self.render_with(
            &templates::builtin_template(COMPACTION_SUMMARY_TEMPLATE)
                .expect("builtin compaction summary template"),
        )
    }

    pub fn render_with(&self, template: &str) -> Result<String, minijinja::Error> {
        templates::render(template, self)
    }

    /// Drops blank entries so a response of blank strings counts as empty
    /// (raw-text fallback) rather than rendering a summary of nothing.
    fn normalize(&mut self) {
        fn blank(s: &str) -> bool {
            s.trim().is_empty()
        }
        for list in [
            &mut self.user_intent,
            &mut self.technical_concepts,
            &mut self.errors_and_fixes,
            &mut self.problem_solving,
            &mut self.user_messages,
            &mut self.pending_tasks,
        ] {
            list.retain(|s| !blank(s));
        }
        for file in &mut self.files {
            if file.key_code.as_deref().is_some_and(blank) {
                file.key_code = None;
            }
        }
        self.files
            .retain(|f| !blank(&f.path) || !blank(&f.summary) || f.key_code.is_some());
        if self.current_work.as_deref().is_some_and(blank) {
            self.current_work = None;
        }
        if self.next_step.as_deref().is_some_and(blank) {
            self.next_step = None;
        }
    }

    fn is_empty(&self) -> bool {
        self.user_intent.is_empty()
            && self.technical_concepts.is_empty()
            && self.files.is_empty()
            && self.errors_and_fixes.is_empty()
            && self.problem_solving.is_empty()
            && self.user_messages.is_empty()
            && self.pending_tasks.is_empty()
            && self.current_work.is_none()
            && self.next_step.is_none()
    }
}

/// Candidate JSON documents in the model's response, tried in order until one
/// parses: after each `</analysis>` terminator (last first) the
/// post-terminator ```json fences (last first) then a leading object, and
/// finally a leading object of the whole text.
///
/// Terminators before the last are retried because the summary JSON may
/// itself quote `</analysis>` (e.g. a session editing compaction prompts),
/// hiding the real terminator from a plain rfind. Such a candidate is
/// accepted only if it contains every later terminator occurrence - proof
/// they were quoted inside it - so a fenced example inside the scratchpad,
/// which precedes the real terminator, can never leak through.
///
/// Every candidate must sit directly at its marker and brace-balance to a
/// close. Anything looser erodes the lossless fallback: JSON merely quoted in
/// a prose response, or a fenced example inside the discarded scratchpad,
/// would silently replace the raw text. Extraction is brace-balanced rather
/// than fence-delimited because string values may legally contain ```, and an
/// unterminated object (output cut off mid-JSON) is not repaired - repair
/// would drop the late, continuation-critical sections that the raw-text
/// fallback preserves.
#[allow(clippy::string_slice)] // All markers are ASCII; indices are byte offsets of ASCII matches.
fn json_candidates(text: &str) -> Vec<&str> {
    const TERMINATOR: &str = "</analysis>";

    let mut cuts: Vec<usize> = text
        .match_indices(TERMINATOR)
        .map(|(idx, _)| idx + TERMINATOR.len())
        .collect();
    if cuts.is_empty() {
        cuts.push(0);
    }

    let mut candidates: Vec<&str> = Vec::new();
    for &cut in cuts.iter().rev() {
        let tail = &text[cut..];
        let later_terminators = tail.matches(TERMINATOR).count();
        candidates.extend(
            fenced_json_blocks(tail)
                .chain(leading_object(tail))
                .filter(|candidate| candidate.matches(TERMINATOR).count() == later_terminators),
        );
    }
    candidates.extend(leading_object(text));
    candidates.dedup();
    candidates
}

/// Every fence is tried, last first, because a string value may itself quote
/// a fenced JSON snippet, and such an embedded fence must not shadow the real
/// one.
#[allow(clippy::string_slice)] // The marker is ASCII; indices are byte offsets of ASCII matches.
fn fenced_json_blocks(text: &str) -> impl Iterator<Item = &str> {
    text.match_indices("```json")
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .filter_map(|(idx, marker)| leading_object(&text[idx + marker.len()..]))
}

#[allow(clippy::string_slice)] // Indices come from char_indices(); slicing is safe.
fn leading_object(text: &str) -> Option<&str> {
    let text = text.trim_start();
    if !text.starts_with('{') {
        return None;
    }

    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for (idx, ch) in text.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[..=idx]);
                }
            }
            _ => {}
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL_RESPONSE: &str = r#"<analysis>
The user asked to fix a bug in parser.rs. I traced it to an off-by-one
in {brace handling} and patched it.
</analysis>

```json
{
  "user_intent": ["Fix the parser bug", "Add a regression test"],
  "technical_concepts": ["off-by-one", "tokenizer"],
  "files": [
    {"path": "src/parser.rs", "summary": "Fixed off-by-one in scan loop", "key_code": "fn scan(&mut self) { .. }"}
  ],
  "errors_and_fixes": ["Panic on empty input, fixed with early return"],
  "problem_solving": ["Root-caused via failing unit test"],
  "user_messages": ["fix the parser bug", "add a test"],
  "pending_tasks": ["Add a regression test"],
  "current_work": "Writing the regression test in tests/parser.rs",
  "next_step": "Finish the regression test"
}
```"#;

    #[test]
    fn parses_fenced_json_after_analysis() {
        let summary = StructuredSummary::parse(FULL_RESPONSE).expect("should parse");
        assert_eq!(
            summary.user_intent,
            vec!["Fix the parser bug", "Add a regression test"]
        );
        assert_eq!(summary.files.len(), 1);
        assert_eq!(summary.files[0].path, "src/parser.rs");
        assert_eq!(
            summary.current_work.as_deref(),
            Some("Writing the regression test in tests/parser.rs")
        );
    }

    #[test]
    fn unusable_responses_fall_back_to_raw_text() {
        for text in [
            // freeform prose, no JSON document
            "Here is a summary of the conversation. The user asked about compaction.",
            // no visible content
            "{}",
            r#"{"notes": "unknown fields alone are not a summary"}"#,
            r#"{"current_work": ""}"#,
            r#"{"files": [{}], "user_intent": [" "]}"#,
            // output cut off mid-JSON: never repaired
            "```json\n{\"user_intent\": [\"Fix the bug\"], \"pending_tasks\": [\"Write tests\", \"Update docs",
            // JSON quoted inside prose is not anchored at a marker
            r#"The session focused on the parser migration. The tracker entry {"current_work": "migrate parser"} is unchanged, and tests still need porting."#,
            "<analysis>reviewing</analysis>\nA prose recap: the config was set to {\"user_intent\": [\"quoted example\"]} per the docs, then the run passed.",
            // a fenced example inside the scratchpad is not the summary
            "<analysis>\nThe target shape is:\n```json\n{\"user_intent\": [\"example only\"]}\n```\nNow let me review the conversation.\n</analysis>\nSorry, I ran out of room and could not produce the summary document.",
            // a quoted terminator inside the scratchpad must not expose its fenced example
            "<analysis>\nThe prompt ends with </analysis> and shows the shape:\n```json\n{\"user_intent\": [\"example only\"]}\n```\nNow let me review the conversation.\n</analysis>\nSorry, I ran out of room and could not produce the summary document.",
        ] {
            assert!(
                StructuredSummary::parse(text).is_none(),
                "should fall back to raw text for: {text}"
            );
        }
    }

    #[test]
    fn embedded_fences_in_string_values_do_not_break_extraction() {
        let text = "```json\n{\"user_intent\": [\"Document the build\"], \"files\": [{\"path\": \"README.md\", \"summary\": \"Added build docs\", \"key_code\": \"```bash\\ncargo build\\n```\"}], \"pending_tasks\": [\"Publish the docs\"]}\n```";
        let summary = StructuredSummary::parse(text).expect("should parse");
        assert_eq!(
            summary.files[0].key_code.as_deref(),
            Some("```bash\ncargo build\n```")
        );
        assert_eq!(summary.pending_tasks, vec!["Publish the docs"]);

        let quoted_json_fence = "```json\n{\"user_intent\": [\"Document the config\"], \"files\": [{\"path\": \"docs/config.md\", \"summary\": \"Added config examples\", \"key_code\": \"```json\\n{\\\"retries\\\": 3}\\n```\"}]}\n```";
        let summary =
            StructuredSummary::parse(quoted_json_fence).expect("should parse via the outer fence");
        assert_eq!(
            summary.files[0].key_code.as_deref(),
            Some("```json\n{\"retries\": 3}\n```")
        );
    }

    #[test]
    fn quoted_terminator_inside_summary_json_does_not_hide_it() {
        let text = "<analysis>\nThe session edited the compaction prompt itself.\n</analysis>\n```json\n{\"user_intent\": [\"Rework the scratchpad prompt\"], \"files\": [{\"path\": \"prompts/compact.md\", \"summary\": \"Tightened the <analysis>...</analysis> instructions\"}]}\n```";
        let summary = StructuredSummary::parse(text).expect("should parse");
        assert_eq!(summary.user_intent, vec!["Rework the scratchpad prompt"]);
        assert_eq!(
            summary.files[0].summary,
            "Tightened the <analysis>...</analysis> instructions"
        );
    }

    #[test]
    fn retries_next_candidate_when_fenced_extraction_fails() {
        let text = "<analysis>the model was told to emit ```json with {braces</analysis>\n{\"user_intent\": [\"Real goal\"]}";
        let summary = StructuredSummary::parse(text).expect("should parse");
        assert_eq!(summary.user_intent, vec!["Real goal"]);
    }

    #[test]
    fn lenient_shapes_are_stringified_not_rejected() {
        let text = r#"{
            "user_intent": "fix the flaky test",
            "errors_and_fixes": [
                {"error": "cursor drifted after replay batch 34", "fix": "bounded mpsc channel"},
                "plain string entry",
                null
            ],
            "pending_tasks": [42],
            "current_work": {"task": "regression test", "status": "in progress"}
        }"#;
        let summary = StructuredSummary::parse(text).expect("should parse leniently");
        assert_eq!(summary.user_intent, vec!["fix the flaky test"]);
        assert_eq!(
            summary.errors_and_fixes,
            vec![
                "error: cursor drifted after replay batch 34; fix: bounded mpsc channel",
                "plain string entry",
            ]
        );
        assert_eq!(summary.pending_tasks, vec!["42"]);
        assert_eq!(
            summary.current_work.as_deref(),
            Some("task: regression test; status: in progress")
        );
    }

    #[test]
    fn file_entries_parse_leniently() {
        let text = r#"{"files": [
            "src/parser.rs",
            {"path": "tests/parser.rs", "summary": "Added regression test"},
            {"path": "src/scan.rs", "summary": 42, "key_code": ["fn a() {}", "fn b() {}"]},
            ""
        ]}"#;
        let summary = StructuredSummary::parse(text).expect("should parse");
        assert_eq!(summary.files.len(), 3);
        assert_eq!(summary.files[0].path, "src/parser.rs");
        assert_eq!(summary.files[0].summary, "");
        assert_eq!(summary.files[1].summary, "Added regression test");
        assert_eq!(summary.files[2].summary, "42");
        assert_eq!(
            summary.files[2].key_code.as_deref(),
            Some("fn a() {}; fn b() {}")
        );
    }

    #[test]
    fn drops_blank_entries_but_keeps_content() {
        let text = r#"{"user_intent": ["", "Fix the bug"], "files": [{"path": "a.rs", "summary": "Patched", "key_code": "  "}], "next_step": " "}"#;
        let summary = StructuredSummary::parse(text).expect("should parse");
        assert_eq!(summary.user_intent, vec!["Fix the bug"]);
        assert_eq!(summary.files[0].key_code, None);
        assert_eq!(summary.next_step, None);
    }

    #[test]
    fn renders_markdown_sections() {
        let summary = StructuredSummary::parse(FULL_RESPONSE).unwrap();
        let rendered = summary.render().expect("should render");
        assert!(rendered.contains("## User Intent"));
        assert!(rendered.contains("- Fix the parser bug"));
        assert!(rendered.contains("### src/parser.rs"));
        assert!(rendered.contains("fn scan(&mut self) { .. }"));
        assert!(rendered.contains("## Next Step"));
    }

    #[test]
    fn render_fences_exceed_backtick_runs_in_key_code() {
        let summary = StructuredSummary {
            files: vec![FileActivity {
                path: "docs/build.md".to_string(),
                summary: "Documented the build".to_string(),
                key_code: Some(
                    "```bash\ncargo build\n```\n````\nnested fence docs\n````".to_string(),
                ),
            }],
            errors_and_fixes: vec!["None".to_string()],
            ..Default::default()
        };
        let rendered = summary.render().expect("should render");
        // key_code's longest backtick run is four, so the fence must be five
        assert_eq!(rendered.matches("\n`````\n").count(), 2);
        let errors_heading = rendered.find("## Errors + Fixes").unwrap();
        let closing_fence = rendered.rfind("\n`````\n").unwrap();
        assert!(errors_heading > closing_fence);
    }
}
