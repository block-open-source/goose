use console::style;
use rmcp::model::ElicitationAction;
use serde_json::Value;
use std::collections::HashMap;
use std::io::{self, BufRead, IsTerminal, Write};

pub struct ElicitationInput {
    pub action: ElicitationAction,
    pub user_data: HashMap<String, Value>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum SelectChoice {
    Value(String),
    Skip,
}

struct SingleSelect<'a> {
    field_name: &'a str,
    description: Option<&'a str>,
    options: Vec<(SelectChoice, String)>,
    initial_value: Option<SelectChoice>,
}

pub fn collect_elicitation_input(message: &str, schema: &Value) -> io::Result<ElicitationInput> {
    if !message.is_empty() {
        println!("\n{}", style(message).cyan());
    }

    let properties = schema.get("properties").and_then(|p| p.as_object());

    if io::stdin().is_terminal() && io::stderr().is_terminal() {
        if let Some(select) = single_select(schema) {
            return prompt_single_select(select);
        }
    }

    let properties = match properties {
        Some(props) if !props.is_empty() => props,
        _ => {
            let prompt = if message.is_empty() {
                "Approve this action?"
            } else {
                "Approve?"
            };
            return match cliclack::confirm(prompt).initial_value(true).interact() {
                Ok(true) => Ok(ElicitationInput {
                    action: ElicitationAction::Accept,
                    user_data: HashMap::new(),
                }),
                Ok(false) => Ok(ElicitationInput {
                    action: ElicitationAction::Decline,
                    user_data: HashMap::new(),
                }),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => Ok(ElicitationInput {
                    action: ElicitationAction::Cancel,
                    user_data: HashMap::new(),
                }),
                Err(e) => Err(e),
            };
        }
    };

    let required: Vec<&str> = schema
        .get("required")
        .and_then(|r| r.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();

    let mut data: HashMap<String, Value> = HashMap::new();

    for (name, field_schema) in properties {
        let is_required = required.contains(&name.as_str());
        let field_type = field_schema
            .get("type")
            .and_then(|t| t.as_str())
            .unwrap_or("string");
        let description = field_schema.get("description").and_then(|d| d.as_str());
        let default = field_schema.get("default");
        let enum_values = field_schema.get("enum").and_then(|e| e.as_array());

        if field_type == "boolean" {
            let label = match description {
                Some(desc) => format!("{} ({})", name, desc),
                None => name.clone(),
            };
            let default_bool = default.and_then(|v| v.as_bool()).unwrap_or(false);

            match cliclack::confirm(&label)
                .initial_value(default_bool)
                .interact()
            {
                Ok(v) => {
                    data.insert(name.clone(), Value::Bool(v));
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {
                    return Ok(ElicitationInput {
                        action: ElicitationAction::Cancel,
                        user_data: HashMap::new(),
                    });
                }
                Err(e) => return Err(e),
            }
            continue;
        }

        if let Some(options) = enum_values {
            let opts: Vec<&str> = options.iter().filter_map(|v| v.as_str()).collect();
            println!("  {}: {}", style("Options").dim(), opts.join(", "));
        }

        print!("{}", style(name).yellow());
        if let Some(desc) = description {
            print!(" {}", style(format!("({})", desc)).dim());
        }
        if is_required {
            print!("{}", style("*").red());
        }
        if let Some(def) = default {
            print!(" {}", style(format!("[{}]", format_default(def))).dim());
        }
        print!(": ");
        io::stdout().flush()?;

        let input = read_line()?;

        if input.is_none() {
            return Ok(ElicitationInput {
                action: ElicitationAction::Cancel,
                user_data: HashMap::new(),
            });
        }
        let input = input.unwrap();

        let value = if input.is_empty() {
            default.cloned()
        } else {
            Some(parse_value(&input, field_type, enum_values))
        };

        if let Some(v) = value {
            if !v.is_null() {
                data.insert(name.clone(), v);
            }
        }

        if is_required && !data.contains_key(name) {
            println!(
                "{}",
                style(format!("Required field '{}' is missing", name)).red()
            );
            return Ok(ElicitationInput {
                action: ElicitationAction::Decline,
                user_data: HashMap::new(),
            });
        }
    }

    println!();
    Ok(ElicitationInput {
        action: ElicitationAction::Accept,
        user_data: data,
    })
}

fn single_select(schema: &Value) -> Option<SingleSelect<'_>> {
    let properties = schema.get("properties")?.as_object()?;
    if properties.len() != 1 {
        return None;
    }

    let (field_name, field_schema) = properties.iter().next()?;
    let description = field_schema.get("description").and_then(Value::as_str);
    let mut options: Vec<(SelectChoice, String)> =
        if let Some(one_of) = field_schema.get("oneOf").and_then(Value::as_array) {
            one_of
                .iter()
                .map(|option| {
                    let value = option.get("const")?.as_str()?;
                    let label = option.get("title").and_then(Value::as_str).unwrap_or(value);
                    Some((SelectChoice::Value(value.to_string()), label.to_string()))
                })
                .collect::<Option<_>>()?
        } else {
            field_schema
                .get("enum")?
                .as_array()?
                .iter()
                .map(|value| {
                    let value = value.as_str()?;
                    Some((SelectChoice::Value(value.to_string()), value.to_string()))
                })
                .collect::<Option<_>>()?
        };

    if options.is_empty() {
        return None;
    }

    let is_required = schema
        .get("required")
        .and_then(Value::as_array)
        .is_some_and(|required| {
            required
                .iter()
                .any(|value| value.as_str() == Some(field_name))
        });
    let default_value = field_schema
        .get("default")
        .and_then(Value::as_str)
        .map(|value| SelectChoice::Value(value.to_string()))
        .filter(|value| options.iter().any(|(option, _)| option == value));

    let initial_value = if !is_required && default_value.is_none() {
        options.push((SelectChoice::Skip, "Skip".to_string()));
        Some(SelectChoice::Skip)
    } else {
        default_value
    };

    Some(SingleSelect {
        field_name,
        description,
        options,
        initial_value,
    })
}

fn prompt_single_select(select: SingleSelect<'_>) -> io::Result<ElicitationInput> {
    let items: Vec<_> = select
        .options
        .iter()
        .map(|(value, label)| (value.clone(), label, ""))
        .collect();
    let label = match select.description {
        Some(desc) => format!("{} ({})", select.field_name, desc),
        None => select.field_name.to_string(),
    };
    let mut prompt = cliclack::select(label).items(&items);
    if let Some(initial_value) = select.initial_value {
        prompt = prompt.initial_value(initial_value);
    }

    match prompt.interact() {
        Ok(SelectChoice::Value(value)) => Ok(ElicitationInput {
            action: ElicitationAction::Accept,
            user_data: HashMap::from([(select.field_name.to_string(), Value::String(value))]),
        }),
        Ok(SelectChoice::Skip) => Ok(ElicitationInput {
            action: ElicitationAction::Accept,
            user_data: HashMap::new(),
        }),
        Err(error) if error.kind() == io::ErrorKind::Interrupted => Ok(ElicitationInput {
            action: ElicitationAction::Cancel,
            user_data: HashMap::new(),
        }),
        Err(error) => Err(error),
    }
}

fn read_line() -> io::Result<Option<String>> {
    read_line_from(&mut io::stdin().lock())
}

fn read_line_from(reader: &mut impl BufRead) -> io::Result<Option<String>> {
    read_line_with(|line| reader.read_line(line))
}

fn read_line_with(
    read: impl FnOnce(&mut String) -> io::Result<usize>,
) -> io::Result<Option<String>> {
    let mut line = String::new();
    match read(&mut line) {
        Ok(0) => Ok(None),
        Ok(_) if line.ends_with('\n') => Ok(Some(line.trim().to_string())),
        Ok(_) => Ok(None),
        Err(e) if e.kind() == io::ErrorKind::Interrupted => Ok(None),
        Err(e) => Err(e),
    }
}

fn format_default(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        _ => value.to_string(),
    }
}

fn parse_value(input: &str, field_type: &str, enum_values: Option<&Vec<Value>>) -> Value {
    if let Some(options) = enum_values {
        let valid: Vec<&str> = options.iter().filter_map(|v| v.as_str()).collect();
        if valid.contains(&input) {
            return Value::String(input.to_string());
        }
        if let Ok(idx) = input.parse::<usize>() {
            if idx > 0 && idx <= valid.len() {
                return Value::String(valid[idx - 1].to_string());
            }
        }
    }

    match field_type {
        "boolean" => {
            let lower = input.to_lowercase();
            Value::Bool(matches!(lower.as_str(), "true" | "yes" | "y" | "1"))
        }
        "integer" => input
            .parse::<i64>()
            .map(|n| Value::Number(n.into()))
            .unwrap_or(Value::Null),
        "number" => input
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        _ => Value::String(input.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Cursor;
    use test_case::test_case;

    #[test]
    fn nonterminal_eof_cancels_elicitation_input() {
        assert_eq!(read_line_from(&mut Cursor::new([])).unwrap(), None);
    }

    #[test]
    fn blank_line_accepts_the_field_default() {
        assert_eq!(
            read_line_from(&mut Cursor::new(b"\n")).unwrap(),
            Some(String::new())
        );
    }

    #[test]
    fn partial_line_at_eof_cancels_elicitation_input() {
        assert_eq!(read_line_from(&mut Cursor::new(b"partial")).unwrap(), None);
    }

    #[test]
    fn interrupted_read_cancels_elicitation_input() {
        assert_eq!(
            read_line_with(|_| Err(io::Error::from(io::ErrorKind::Interrupted))).unwrap(),
            None
        );
    }

    #[test]
    fn builds_required_enum_select_with_default() {
        let schema = json!({
            "type": "object",
            "properties": {
                "color": {
                    "type": "string",
                    "enum": ["red", "green"],
                    "default": "green"
                }
            },
            "required": ["color"]
        });

        let select = single_select(&schema).unwrap();

        assert_eq!(select.field_name, "color");
        assert_eq!(
            select.options,
            vec![
                (SelectChoice::Value("red".to_string()), "red".to_string()),
                (
                    SelectChoice::Value("green".to_string()),
                    "green".to_string()
                )
            ]
        );
        assert_eq!(
            select.initial_value,
            Some(SelectChoice::Value("green".to_string()))
        );
    }

    #[test]
    fn optional_select_defaults_to_skip() {
        let schema = json!({
            "type": "object",
            "properties": {
                "color": { "type": "string", "enum": ["", "green"] }
            }
        });

        let select = single_select(&schema).unwrap();

        assert_eq!(select.initial_value, Some(SelectChoice::Skip));
        assert_eq!(select.options.last().unwrap().0, SelectChoice::Skip);
        assert_eq!(
            select.options.first().unwrap().0,
            SelectChoice::Value(String::new())
        );
    }

    #[test]
    fn uses_one_of_titles_as_labels() {
        let schema = json!({
            "type": "object",
            "properties": {
                "size": {
                    "oneOf": [
                        { "const": "s", "title": "Small" },
                        { "const": "l", "title": "Large" }
                    ]
                }
            },
            "required": ["size"]
        });

        let select = single_select(&schema).unwrap();

        assert_eq!(select.options[0].0, SelectChoice::Value("s".to_string()));
        assert_eq!(select.options[0].1, "Small");
        assert_eq!(select.options[1].0, SelectChoice::Value("l".to_string()));
        assert_eq!(select.options[1].1, "Large");
    }

    #[test_case(json!({}); "missing properties")]
    #[test_case(json!({ "properties": {} }); "empty properties")]
    #[test_case(json!({
        "properties": {
            "first": { "enum": ["a"] },
            "second": { "enum": ["b"] }
        }
    }); "multiple properties")]
    #[test_case(json!({
        "properties": { "choice": { "enum": ["a", 2] } }
    }); "non-string enum value")]
    #[test_case(json!({
        "properties": {
            "choice": { "oneOf": [{ "const": "a" }, { "type": "string" }] }
        }
    }); "oneOf branch without const")]
    fn unsupported_schema_does_not_build_select(schema: Value) {
        assert!(single_select(&schema).is_none());
    }
}
