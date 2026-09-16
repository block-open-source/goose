use anyhow::Result;
use console::{measure_text_width, Term};
use goose::skills::list_installed_skills;
use goose::token_counter::create_token_counter;
use unicode_segmentation::UnicodeSegmentation;

const DESCRIPTION_PREVIEW_CHARS: usize = 50;
const SEPARATOR: &str = " | ";
const MIN_DESCRIPTION_WIDTH: usize = 4;
const MIN_LOCATION_WIDTH: usize = 4;
const NAME_HEADER: &str = "Name";
const DESCRIPTION_HEADER: &str = "Description";
const DESCRIPTION_TOKENS_HEADER: &str = "Description tokens";
const CONTENT_TOKENS_HEADER: &str = "Content tokens";
const LOCATION_HEADER: &str = "Location";

struct SkillRow {
    name: String,
    description: String,
    description_tokens: String,
    content_tokens: String,
    location: String,
}

struct ColumnWidths {
    name: usize,
    description: usize,
    description_tokens: usize,
    content_tokens: usize,
    location: usize,
}

pub async fn handle_skills_list() -> Result<()> {
    let cwd = std::env::current_dir()?;
    let terminal_width = terminal_width();
    let token_counter = create_token_counter().await.map_err(anyhow::Error::msg)?;
    let mut skills = list_installed_skills(Some(&cwd));
    skills.sort_by(|a, b| a.name.cmp(&b.name));

    let rows = skills
        .iter()
        .map(|skill| SkillRow {
            name: skill.name.clone(),
            description: description_preview(&skill.description),
            description_tokens: token_counter.count_tokens(&skill.description).to_string(),
            content_tokens: token_counter.count_tokens(&skill.content).to_string(),
            location: skill.path.clone(),
        })
        .collect::<Vec<_>>();
    let widths = column_widths(&rows, terminal_width);

    println!("{}", header_line(&widths, terminal_width));
    for row in rows {
        println!("{}", skill_line(&row, &widths, terminal_width));
    }

    Ok(())
}

fn terminal_width() -> Option<usize> {
    Term::stdout()
        .size_checked()
        .map(|(_height, width)| width as usize)
}

fn column_widths(rows: &[SkillRow], max_display_width: Option<usize>) -> ColumnWidths {
    let description_tokens = max_width(
        DESCRIPTION_TOKENS_HEADER,
        rows.iter().map(|row| row.description_tokens.as_str()),
    );
    let content_tokens = max_width(
        CONTENT_TOKENS_HEADER,
        rows.iter().map(|row| row.content_tokens.as_str()),
    );
    let longest_name = max_width(NAME_HEADER, rows.iter().map(|row| row.name.as_str()));
    let description = max_width(
        DESCRIPTION_HEADER,
        rows.iter().map(|row| row.description.as_str()),
    );
    let location = max_width(
        LOCATION_HEADER,
        rows.iter().map(|row| row.location.as_str()),
    );

    let Some(width) = max_display_width else {
        return ColumnWidths {
            name: longest_name,
            description,
            description_tokens,
            content_tokens,
            location,
        };
    };

    let separator_width = measure_text_width(SEPARATOR) * 4;
    let available_width = width.saturating_sub(separator_width);
    let dynamic_width = available_width.saturating_sub(description_tokens + content_tokens);

    let name =
        longest_name.min(dynamic_width.saturating_sub(MIN_DESCRIPTION_WIDTH + MIN_LOCATION_WIDTH));
    let remaining_after_name = dynamic_width.saturating_sub(name);
    let description = description.min(remaining_after_name.saturating_sub(MIN_LOCATION_WIDTH));
    let remaining_after_description = remaining_after_name.saturating_sub(description);
    let location = location.min(remaining_after_description);

    ColumnWidths {
        name,
        description,
        description_tokens,
        content_tokens,
        location,
    }
}

fn max_width<'a>(header: &str, values: impl Iterator<Item = &'a str>) -> usize {
    values
        .map(measure_text_width)
        .chain(std::iter::once(measure_text_width(header)))
        .max()
        .unwrap_or(0)
}

fn header_line(widths: &ColumnWidths, max_display_width: Option<usize>) -> String {
    let line = format_line(
        NAME_HEADER,
        DESCRIPTION_HEADER,
        DESCRIPTION_TOKENS_HEADER,
        CONTENT_TOKENS_HEADER,
        LOCATION_HEADER,
        widths,
    );

    match max_display_width {
        Some(width) => truncate_to_display_width(&line, width),
        None => line,
    }
}

fn skill_line(row: &SkillRow, widths: &ColumnWidths, max_display_width: Option<usize>) -> String {
    let line = format_line(
        &row.name,
        &row.description,
        &row.description_tokens,
        &row.content_tokens,
        &row.location,
        widths,
    );

    match max_display_width {
        Some(width) => truncate_to_display_width(&line, width),
        None => line,
    }
}

fn format_line(
    name: &str,
    description: &str,
    description_tokens: &str,
    content_tokens: &str,
    location: &str,
    widths: &ColumnWidths,
) -> String {
    format!(
        "{}{}{}{}{}{}{}{}{}",
        pad_to_display_width(&truncate_to_display_width(name, widths.name), widths.name),
        SEPARATOR,
        pad_to_display_width(
            &truncate_to_display_width(description, widths.description),
            widths.description
        ),
        SEPARATOR,
        pad_to_display_width(description_tokens, widths.description_tokens),
        SEPARATOR,
        pad_to_display_width(content_tokens, widths.content_tokens),
        SEPARATOR,
        pad_to_display_width(
            &truncate_to_display_width(location, widths.location),
            widths.location
        ),
    )
}

fn description_preview(description: &str) -> String {
    let normalized = description.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_to_chars(&normalized, DESCRIPTION_PREVIEW_CHARS)
}

fn truncate_to_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }

    if max_chars <= 3 {
        return ".".repeat(max_chars);
    }

    let mut output = text.chars().take(max_chars - 3).collect::<String>();
    output.push_str("...");
    output
}

fn truncate_to_display_width(text: &str, max_width: usize) -> String {
    if measure_text_width(text) <= max_width {
        return text.to_string();
    }

    if max_width <= 3 {
        return ".".repeat(max_width);
    }

    let suffix_width = measure_text_width("...");
    let mut output = String::new();
    let mut output_width = 0;

    for grapheme in text.graphemes(true) {
        let grapheme_width = measure_text_width(grapheme);
        if output_width + grapheme_width + suffix_width > max_width {
            break;
        }
        output.push_str(grapheme);
        output_width += grapheme_width;
    }

    output.push_str("...");
    output
}

fn pad_to_display_width(text: &str, width: usize) -> String {
    let padding = width.saturating_sub(measure_text_width(text));
    format!("{}{}", text, " ".repeat(padding))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_to_display_width_keeps_grapheme_clusters_intact() {
        assert_eq!(truncate_to_display_width("🏳️‍🌈abcd", 5), "🏳️‍🌈...");
        assert_eq!(truncate_to_display_width("👩‍💻abcd", 5), "👩‍💻...");
        assert_eq!(truncate_to_display_width("a\u{301}bcde", 4), "a\u{301}...");
    }

    #[test]
    fn truncate_to_display_width_drops_cluster_that_cannot_fit() {
        assert_eq!(truncate_to_display_width("🏳️‍🌈abc", 4), "...");
        assert_eq!(truncate_to_display_width("界界界", 5), "界...");
    }
}
