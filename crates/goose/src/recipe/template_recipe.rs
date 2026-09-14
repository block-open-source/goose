use std::{
    collections::{HashMap, HashSet},
    path::{Component, Path},
};

use crate::recipe::{Recipe, BUILT_IN_RECIPE_DIR_PARAM};
use anyhow::Result;
use minijinja::{Environment, UndefinedBehavior};
use regex::Regex;

const CURRENT_TEMPLATE_NAME: &str = "recipe";
const OPEN_BRACE: &str = "{{";
const CLOSE_BRACE: &str = "}}";

pub(crate) struct ParsedRecipeTemplate {
    recipe: Recipe,
    template_variables: HashSet<String>,
    environment: Environment<'static>,
}

impl ParsedRecipeTemplate {
    pub(crate) fn recipe(&self) -> &Recipe {
        &self.recipe
    }

    pub(crate) fn template_variables(&self) -> &HashSet<String> {
        &self.template_variables
    }

    pub(crate) fn into_recipe(self) -> Recipe {
        self.recipe
    }

    pub(crate) fn render(
        mut self,
        params: &HashMap<String, String>,
    ) -> Result<(String, HashSet<String>)> {
        self.environment
            .set_undefined_behavior(UndefinedBehavior::Strict);
        self.environment.set_loader(|_| Ok(None));
        let template = self.environment.get_template(CURRENT_TEMPLATE_NAME)?;
        let rendered = template
            .render(params)
            .map_err(|error| anyhow::anyhow!("Failed to render the recipe {error}"))?;
        Ok((rendered, self.template_variables))
    }
}

fn preprocess_template_variables(content: &str) -> Result<String> {
    let all_template_variables = extract_template_variables(content);
    let complex_template_variables = filter_complex_variables(&all_template_variables);
    let unparsable_template_variables = filter_unparseable_variables(&complex_template_variables)?;
    replace_unparseable_vars_with_raw(content, &unparsable_template_variables)
}

fn extract_template_variables(content: &str) -> Vec<String> {
    let template_var_re = Regex::new(r"\{\{(.*?)\}\}").unwrap();
    template_var_re
        .captures_iter(content)
        .map(|cap| cap[1].to_string())
        .collect()
}

// filter out variables that are not only alphanumeric and underscores
fn filter_complex_variables(template_variables: &[String]) -> Vec<String> {
    let valid_var_re = Regex::new(r"^\s*[a-zA-Z_][a-zA-Z0-9_]*\s*$").unwrap();
    template_variables
        .iter()
        .filter(|var| !valid_var_re.is_match(var))
        .cloned()
        .collect()
}

fn filter_unparseable_variables(template_variables: &[String]) -> Result<Vec<String>> {
    let mut vars_to_convert = Vec::new();

    for var in template_variables {
        let trimmed = var.trim();

        if trimmed.starts_with('\'') || trimmed.starts_with('"') {
            continue;
        }

        let mut env = Environment::new();
        env.set_undefined_behavior(UndefinedBehavior::Lenient);

        let test_template = format!(
            "{open}{content}{close}",
            open = OPEN_BRACE,
            content = var,
            close = CLOSE_BRACE
        );
        if env.template_from_str(&test_template).is_err() {
            vars_to_convert.push(var.clone());
        }
    }

    Ok(vars_to_convert)
}

fn replace_unparseable_vars_with_raw(
    content: &str,
    unparsable_template_variables: &[String],
) -> Result<String> {
    let mut result = content.to_string();

    for var in unparsable_template_variables {
        let pattern = format!(
            "{open}{content}{close}",
            open = OPEN_BRACE,
            content = var,
            close = CLOSE_BRACE
        );
        let replacement = format!(
            "{{% raw %}}{open}{content}{close}{{% endraw %}}",
            open = OPEN_BRACE,
            close = CLOSE_BRACE,
            content = var
        );
        result = result.replace(&pattern, &replacement);
    }

    Ok(result)
}

pub fn render_recipe_content_with_params(
    content: &str,
    params: &HashMap<String, String>,
) -> Result<String> {
    let empty_quotes = Regex::new(r#":\s*"""#).unwrap();
    let content_with_empty_quotes_replaced = empty_quotes.replace_all(content, ": ''");
    let content_with_safe_variables =
        preprocess_template_variables(&content_with_empty_quotes_replaced)?;

    let env = add_template_in_env(
        &content_with_safe_variables,
        params.get(BUILT_IN_RECIPE_DIR_PARAM).cloned(),
        UndefinedBehavior::Strict,
    )?;
    let template = env.get_template(CURRENT_TEMPLATE_NAME).unwrap();
    let rendered_content = template
        .render(params)
        .map_err(|e| anyhow::anyhow!("Failed to render the recipe {}", e))?;
    Ok(rendered_content)
}

fn add_template_in_env(
    content: &str,
    recipe_dir: Option<String>,
    undefined_behavior: UndefinedBehavior,
) -> Result<Environment<'static>> {
    let mut env = minijinja::Environment::new();
    env.set_undefined_behavior(undefined_behavior);

    if let Some(recipe_dir) = recipe_dir {
        env.set_loader(move |name| {
            load_template_from_recipe_dir(Path::new(recipe_dir.as_str()), name)
        });
    }

    env.add_template_owned(CURRENT_TEMPLATE_NAME, content.to_string())?;
    Ok(env)
}

fn load_template_from_recipe_dir(
    recipe_dir: &Path,
    name: &str,
) -> Result<Option<String>, minijinja::Error> {
    if Path::new(name).components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(minijinja::Error::new(
            minijinja::ErrorKind::InvalidOperation,
            "template path must stay within the recipe directory",
        ));
    }

    let recipe_dir = match std::fs::canonicalize(recipe_dir) {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(template_loader_error(error)),
    };
    let path = match std::fs::canonicalize(recipe_dir.join(name)) {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(template_loader_error(error)),
    };

    if !path.starts_with(&recipe_dir) {
        return Err(minijinja::Error::new(
            minijinja::ErrorKind::InvalidOperation,
            "template path must stay within the recipe directory",
        ));
    }

    match std::fs::read_to_string(path) {
        Ok(content) => Ok(Some(content)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(template_loader_error(error)),
    }
}

fn template_loader_error(error: std::io::Error) -> minijinja::Error {
    minijinja::Error::new(
        minijinja::ErrorKind::InvalidOperation,
        "could not read template",
    )
    .with_source(error)
}

fn get_env_with_template_variables(
    content: &str,
    recipe_dir: Option<String>,
    undefined_behavior: UndefinedBehavior,
) -> Result<(Environment<'static>, HashSet<String>)> {
    let env = add_template_in_env(content, recipe_dir, undefined_behavior)?;
    let template_variables = {
        let template = env.get_template(CURRENT_TEMPLATE_NAME).unwrap();
        let captured = template.render_captured(())?;
        let state = captured.state();
        let mut vars = HashSet::new();
        for (_, tmpl) in state.env().templates() {
            vars.extend(tmpl.undeclared_variables(true));
        }
        vars
    };
    Ok((env, template_variables))
}

fn uses_template_inheritance(content: &str) -> bool {
    let re = Regex::new(r"\{%-?\s*(extends|include)").unwrap();
    re.is_match(content)
}

pub fn parse_recipe_content(
    content: &str,
    recipe_dir: Option<String>,
) -> Result<(Recipe, HashSet<String>)> {
    let parsed = parse_recipe_template(content, recipe_dir)?;
    Ok((parsed.recipe, parsed.template_variables))
}

pub(crate) fn parse_recipe_template(
    content: &str,
    recipe_dir: Option<String>,
) -> Result<ParsedRecipeTemplate> {
    let preprocessed_content = preprocess_template_variables(content)?;

    let (env, template_variables) = get_env_with_template_variables(
        &preprocessed_content,
        recipe_dir,
        UndefinedBehavior::Lenient,
    )?;
    let template = env.get_template(CURRENT_TEMPLATE_NAME).unwrap();

    // Detect if template uses inheritance or includes
    let recipe_content = if uses_template_inheritance(&preprocessed_content) {
        // Must render to resolve inheritance
        template
            .render(())
            .map_err(|e| anyhow::anyhow!("Failed to parse the recipe {}", e))?
    } else {
        // Preserve conditionals and variables as-is
        preprocessed_content
    };

    let recipe = Recipe::from_content(&recipe_content)?;
    Ok(ParsedRecipeTemplate {
        recipe,
        template_variables,
        environment: env,
    })
}

#[cfg(test)]
mod tests {
    mod parse_recipe_content_tests {
        use crate::recipe::{validate_recipe::validate_recipe_template_from_content, Recipe};

        #[test]
        fn preserves_empty_string_json_in_instruction_block_scalar() {
            let content = r#"version: 1.0.0
title: Empty string example
description: Empty string example
instructions: |
  Return this JSON unchanged:
  {"value": ""}
  Example: ""
"#;

            let expected = Recipe::from_content(content).unwrap();
            let recipe = validate_recipe_template_from_content(content, None).unwrap();

            assert_eq!(recipe.instructions, expected.instructions);
        }

        #[test]
        fn preserves_empty_string_in_bare_sequence_block_scalar() {
            let content = r#"version: 1.0.0
title: Activity empty string example
description: Activity empty string example
instructions: Keep activities unchanged
activities:
  - |
    value: ""
"#;

            let expected = Recipe::from_content(content).unwrap();
            let recipe = validate_recipe_template_from_content(content, None).unwrap();

            assert_eq!(recipe.activities, expected.activities);
        }

        #[test]
        fn preserves_empty_strings_in_multiline_quoted_instructions() {
            let single_quoted = r#"version: 1.0.0
title: Single quoted empty string example
description: Single quoted empty string example
instructions: 'Return:
  value: ""
  done'
"#;
            let double_quoted = r#"version: 1.0.0
title: Double quoted empty string example
description: Double quoted empty string example
instructions: "Return:
  value: \"\"
  done"
"#;

            for content in [single_quoted, double_quoted] {
                let expected = Recipe::from_content(content).unwrap();
                let recipe = validate_recipe_template_from_content(content, None).unwrap();

                assert_eq!(recipe.instructions, expected.instructions);
            }
        }
    }

    mod template_loader_tests {
        use std::{collections::HashMap, fs};

        use crate::recipe::{
            template_recipe::render_recipe_content_with_params, BUILT_IN_RECIPE_DIR_PARAM,
        };

        fn render(content: &str, recipe_dir: &std::path::Path) -> anyhow::Result<String> {
            let params = HashMap::from([(
                BUILT_IN_RECIPE_DIR_PARAM.to_string(),
                recipe_dir.display().to_string(),
            )]);
            render_recipe_content_with_params(content, &params)
        }

        #[test]
        fn rejects_absolute_template_paths() {
            let temp_dir = tempfile::tempdir().unwrap();
            let recipe_dir = temp_dir.path().join("recipe");
            let secret = temp_dir.path().join("secret.txt");
            fs::create_dir(&recipe_dir).unwrap();
            fs::write(&secret, "secret").unwrap();

            let content = format!(r#"{{% include "{}" %}}"#, secret.display());

            assert!(render(&content, &recipe_dir).is_err());
        }

        #[test]
        fn rejects_parent_directory_traversal() {
            let temp_dir = tempfile::tempdir().unwrap();
            let recipe_dir = temp_dir.path().join("recipe");
            fs::create_dir(&recipe_dir).unwrap();
            fs::write(temp_dir.path().join("secret.txt"), "secret").unwrap();

            assert!(render(r#"{% include "../secret.txt" %}"#, &recipe_dir).is_err());
        }

        #[cfg(unix)]
        #[test]
        fn rejects_symlinks_outside_recipe_directory() {
            use std::os::unix::fs::symlink;

            let temp_dir = tempfile::tempdir().unwrap();
            let recipe_dir = temp_dir.path().join("recipe");
            let outside_dir = temp_dir.path().join("outside");
            fs::create_dir(&recipe_dir).unwrap();
            fs::create_dir(&outside_dir).unwrap();
            fs::write(outside_dir.join("secret.txt"), "secret").unwrap();
            symlink(&outside_dir, recipe_dir.join("linked")).unwrap();

            assert!(render(r#"{% include "linked/secret.txt" %}"#, &recipe_dir).is_err());
        }

        #[test]
        fn renders_nested_templates_within_recipe_directory() {
            let temp_dir = tempfile::tempdir().unwrap();
            let recipe_dir = temp_dir.path().join("recipe");
            let nested_dir = recipe_dir.join("nested");
            fs::create_dir_all(&nested_dir).unwrap();
            fs::write(nested_dir.join("partial.txt"), "nested content").unwrap();

            let rendered = render(r#"{% include "nested/partial.txt" %}"#, &recipe_dir).unwrap();

            assert_eq!(rendered, "nested content");
        }
    }

    mod render_content_with_params_tests {
        use std::collections::HashMap;

        use crate::recipe::template_recipe::render_recipe_content_with_params;

        #[test]
        fn test_render_content_with_params() {
            // Test basic parameter substitution
            let content = "Hello {{ name }}!";
            let params = HashMap::from([
                ("recipe_dir".to_string(), "some_dir".to_string()),
                ("name".to_string(), "World".to_string()),
            ]);
            let result = render_recipe_content_with_params(content, &params).unwrap();
            assert_eq!(result, "Hello World!");

            // Test empty parameter substitution
            let content = "Hello {{ empty }}!";
            let params = HashMap::from([
                ("recipe_dir".to_string(), "some_dir".to_string()),
                ("empty".to_string(), "".to_string()),
            ]);
            let result = render_recipe_content_with_params(content, &params).unwrap();
            assert_eq!(result, "Hello !");

            // Test multiple parameters
            let content = "{{ greeting }} {{ name }}!";
            let params = HashMap::from([
                ("recipe_dir".to_string(), "some_dir".to_string()),
                ("greeting".to_string(), "Hi".to_string()),
                ("name".to_string(), "Alice".to_string()),
            ]);
            let result = render_recipe_content_with_params(content, &params).unwrap();
            assert_eq!(result, "Hi Alice!");

            // Test missing parameter results in error
            let content = "Hello {{ missing }}!";
            let params = HashMap::from([("recipe_dir".to_string(), "some_dir".to_string())]);
            let err = render_recipe_content_with_params(content, &params).unwrap_err();
            let error_msg = err.to_string();
            assert!(error_msg.contains("Failed to render the recipe"));

            // Test invalid template syntax results in error
            let content = "Hello {{ unclosed";
            let params = HashMap::from([("recipe_dir".to_string(), "some_dir".to_string())]);
            let err = render_recipe_content_with_params(content, &params).unwrap_err();
            assert!(err.to_string().contains("unexpected end of input"));
        }

        #[test]
        fn test_builtin_filters_are_registered() {
            // Regression test for #11301: minijinja's `builtins` feature (where
            // indent/upper/trim/etc. live) was dropped from Cargo.toml, so every
            // documented recipe filter except safe/escape/e failed as "unknown filter".
            let content = "{{ raw_data | indent(2) }}";
            let params = HashMap::from([
                ("recipe_dir".to_string(), "some_dir".to_string()),
                ("raw_data".to_string(), "line one\nline two".to_string()),
            ]);
            let result = render_recipe_content_with_params(content, &params).unwrap();
            assert_eq!(result, "line one\n  line two");

            let content = "{{ raw_data | upper }} / {{ raw_data | trim }}";
            let params = HashMap::from([
                ("recipe_dir".to_string(), "some_dir".to_string()),
                ("raw_data".to_string(), "  hi  ".to_string()),
            ]);
            let result = render_recipe_content_with_params(content, &params).unwrap();
            assert_eq!(result, "  HI   / hi");
        }

        #[test]
        fn test_render_content_with_spaced_variables() {
            let content = "Hello {{hf model org}}_{{hf model name}}!";
            let params = HashMap::from([("recipe_dir".to_string(), "some_dir".to_string())]);
            let result = render_recipe_content_with_params(content, &params).unwrap();
            assert_eq!(result, "Hello {{hf model org}}_{{hf model name}}!");

            let content = "Hello {{hf model org}_{hf model name}}!";
            let params = HashMap::from([("recipe_dir".to_string(), "some_dir".to_string())]);
            let result = render_recipe_content_with_params(content, &params).unwrap();
            assert_eq!(result, "Hello {{hf model org}_{hf model name}}!");

            let content = "Hello {{valid_var}}!";
            let params = HashMap::from([
                ("recipe_dir".to_string(), "some_dir".to_string()),
                ("valid_var".to_string(), "World".to_string()),
            ]);
            let result = render_recipe_content_with_params(content, &params).unwrap();
            assert_eq!(result, "Hello World!");

            let content = "{{valid_var}} and {{invalid var}}";
            let params = HashMap::from([
                ("recipe_dir".to_string(), "some_dir".to_string()),
                ("valid_var".to_string(), "Hello".to_string()),
            ]);
            let result = render_recipe_content_with_params(content, &params).unwrap();
            assert_eq!(result, "Hello and {{invalid var}}");
        }

        #[test]
        fn test_empty_prompt() {
            let content = r#"
prompt: ""
name: "Simple Recipe"
description: "A test recipe"
"#;
            let params = HashMap::from([("recipe_dir".to_string(), "test_dir".to_string())]);
            let result = render_recipe_content_with_params(content, &params).unwrap();

            assert!(result.contains("prompt: ''"));
            assert!(!result.contains(r#"prompt: "\"\"""#)); // Should not contain escaped quotes

            assert!(result.contains(r#"name: "Simple Recipe""#));
        }

        #[test]
        fn test_jinja_escape_syntax() {
            let content = r#"{{'{{param_key}}'}}"#;
            let params = HashMap::from([("recipe_dir".to_string(), "test_dir".to_string())]);
            let result = render_recipe_content_with_params(content, &params).unwrap();
            assert_eq!(result, "{{param_key}}");
        }
    }
}
