use include_dir::{include_dir, Dir};
use minijinja::{Environment, Error as MiniJinjaError, Value as MJValue};
use serde::Serialize;

static PROMPTS: Dir = include_dir!("$CARGO_MANIFEST_DIR/src/prompts");

pub const COMPACTION_TEMPLATE: &str = "compaction.md";
pub const COMPACTION_SUMMARY_TEMPLATE: &str = "compaction_summary.md";
pub const COMPACTION_PREFIX_TEMPLATE: &str = "compaction_prefix.md";

pub fn builtin_template(name: &str) -> Option<String> {
    PROMPTS
        .get_file(name)
        .map(|file| String::from_utf8_lossy(file.contents()).to_string())
}

/// Prompt sources for a compaction run, letting callers substitute
/// user-customized templates for the built-in ones.
#[derive(Debug, Clone)]
pub struct Templates {
    pub compaction: String,
    pub summary: String,
}

impl Default for Templates {
    fn default() -> Self {
        Self {
            compaction: builtin_template(COMPACTION_TEMPLATE).expect("builtin compaction template"),
            summary: builtin_template(COMPACTION_SUMMARY_TEMPLATE)
                .expect("builtin compaction summary template"),
        }
    }
}

fn code_fence(code: String) -> String {
    let longest_run = code
        .chars()
        .fold((0usize, 0usize), |(max, run), c| {
            if c == '`' {
                (max.max(run + 1), run + 1)
            } else {
                (max, 0)
            }
        })
        .0;
    let fence = "`".repeat((longest_run + 1).max(3));
    format!("{fence}\n{}\n{fence}", code.trim_end_matches('\n'))
}

pub fn render<T: Serialize>(template: &str, context: &T) -> Result<String, MiniJinjaError> {
    let mut env = Environment::new();
    env.set_trim_blocks(true);
    env.set_lstrip_blocks(true);
    env.add_filter("code_fence", code_fence);
    env.add_template("template", template)?;
    let rendered = env
        .get_template("template")?
        .render(MJValue::from_serialize(context))?;
    Ok(rendered.trim().to_string())
}
