use crate::recipe::read_recipe_file_content::RecipeFile;
use crate::recipe::template_recipe::{
    parse_recipe_content, parse_recipe_template, ParsedRecipeTemplate,
};
use crate::recipe::{
    Recipe, RecipeParameter, RecipeParameterInputType, RecipeParameterRequirement,
    BUILT_IN_RECIPE_DIR_PARAM,
};
use anyhow::Result;
use std::collections::{HashMap, HashSet};

pub(crate) struct ValidatedRecipeTemplate {
    parsed: ParsedRecipeTemplate,
}

impl ValidatedRecipeTemplate {
    pub(crate) fn recipe(&self) -> &Recipe {
        self.parsed.recipe()
    }

    pub(crate) fn into_recipe(self) -> Recipe {
        self.parsed.into_recipe()
    }

    pub(crate) fn render(self, params: &HashMap<String, String>) -> Result<Recipe> {
        let (rendered_content, template_variables) = self.parsed.render(params)?;
        let recipe = Recipe::from_content(&rendered_content)?;
        validate_recipe_parameters(&recipe, &template_variables)?;
        validate_recipe_non_parameter_invariants(&recipe)?;
        Ok(recipe)
    }
}

pub fn parse_and_validate_parameters(
    recipe_file_content: &str,
    recipe_dir_str: Option<String>,
) -> Result<Recipe> {
    let (recipe_template, template_variables) =
        parse_recipe_content(recipe_file_content, recipe_dir_str)?;
    validate_recipe_parameters(&recipe_template, &template_variables)?;
    Ok(recipe_template)
}

fn validate_recipe_parameters(recipe: &Recipe, template_variables: &HashSet<String>) -> Result<()> {
    validate_optional_parameters(&recipe.parameters)?;
    validate_parameters_in_template(&recipe.parameters, template_variables)
}

fn validate_json_schema(schema: &serde_json::Value) -> Result<()> {
    let schema_object = schema
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("JSON schema must be an object"))?;
    if schema_object.is_empty() {
        return Err(anyhow::anyhow!("Empty JSON schema is not allowed"));
    }
    jsonschema::validator_for(schema)
        .map(|_| ())
        .map_err(|error| anyhow::anyhow!("JSON schema validation failed: {error}"))
}

pub fn validate_recipe_template_from_file(recipe_file: &RecipeFile) -> Result<Recipe> {
    let recipe_dir = recipe_file
        .parent_dir
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Error getting recipe directory"))?
        .to_string();

    validate_recipe_template_from_content(&recipe_file.content, Some(recipe_dir))
}

pub fn validate_recipe_template_from_content(
    recipe_content: &str,
    recipe_dir: Option<String>,
) -> Result<Recipe> {
    Ok(validate_recipe_template(recipe_content, recipe_dir)?.into_recipe())
}

pub(crate) fn validate_recipe_template(
    recipe_content: &str,
    recipe_dir: Option<String>,
) -> Result<ValidatedRecipeTemplate> {
    let parsed = parse_recipe_template(recipe_content, recipe_dir)?;
    validate_recipe_parameters(parsed.recipe(), parsed.template_variables())?;
    validate_recipe_non_parameter_invariants(parsed.recipe())?;

    Ok(ValidatedRecipeTemplate { parsed })
}

pub(crate) fn validate_recipe_non_parameter_invariants(recipe: &Recipe) -> Result<()> {
    validate_prompt_or_instructions(recipe)?;
    validate_retry_config(recipe)?;
    if let Some(response) = &recipe.response {
        if let Some(json_schema) = &response.json_schema {
            validate_json_schema(json_schema)?;
        }
    }

    Ok(())
}

fn validate_retry_config(recipe: &Recipe) -> Result<()> {
    if let Some(ref retry_config) = recipe.retry {
        if let Err(validation_error) = retry_config.validate() {
            return Err(anyhow::anyhow!(
                "Invalid retry configuration: {}",
                validation_error
            ));
        }
    }
    Ok(())
}

fn validate_prompt_or_instructions(recipe: &Recipe) -> Result<()> {
    let has_instructions = recipe
        .instructions
        .as_ref()
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false);
    let has_prompt = recipe
        .prompt
        .as_ref()
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false);

    if has_instructions || has_prompt {
        return Ok(());
    }

    Err(anyhow::anyhow!(
        "Recipe must specify at least one of `instructions` or `prompt`."
    ))
}

fn validate_parameters_in_template(
    recipe_parameters: &Option<Vec<RecipeParameter>>,
    template_variables: &HashSet<String>,
) -> Result<()> {
    let mut template_variables = template_variables.clone();
    template_variables.remove(BUILT_IN_RECIPE_DIR_PARAM);

    let mut param_keys = HashSet::new();
    for parameter in recipe_parameters.as_deref().unwrap_or_default() {
        if !param_keys.insert(parameter.key.clone()) {
            return Err(anyhow::anyhow!(
                "Duplicate parameter definition: {}.",
                parameter.key
            ));
        }
    }

    let missing_keys = template_variables
        .difference(&param_keys)
        .collect::<Vec<_>>();

    let extra_keys = param_keys
        .difference(&template_variables)
        .collect::<Vec<_>>();

    if missing_keys.is_empty() && extra_keys.is_empty() {
        return Ok(());
    }

    let mut message = String::new();

    if !missing_keys.is_empty() {
        message.push_str(&format!(
            "Missing definitions for parameters in the recipe file: {}.",
            missing_keys
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    if !extra_keys.is_empty() {
        message.push_str(&format!(
            "\nUnnecessary parameter definitions: {}.",
            extra_keys
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    Err(anyhow::anyhow!("{}", message.trim_end()))
}

fn validate_optional_parameters(parameters: &Option<Vec<RecipeParameter>>) -> Result<()> {
    let empty_params = vec![];
    let params = parameters.as_ref().unwrap_or(&empty_params);

    let file_params_with_defaults: Vec<String> = params
        .iter()
        .filter(|p| matches!(p.input_type, RecipeParameterInputType::File) && p.default.is_some())
        .map(|p| p.key.clone())
        .collect();

    if !file_params_with_defaults.is_empty() {
        return Err(anyhow::anyhow!("File parameters cannot have default values to avoid importing sensitive user files: {}", file_params_with_defaults.join(", ")));
    }

    let optional_params_without_default_values: Vec<String> = params
        .iter()
        .filter(|p| {
            matches!(p.requirement, RecipeParameterRequirement::Optional) && p.default.is_none()
        })
        .map(|p| p.key.clone())
        .collect();

    if optional_params_without_default_values.is_empty() {
        Ok(())
    } else {
        Err(anyhow::anyhow!("Optional parameters missing default values in the recipe: {}. Please provide defaults.", optional_params_without_default_values.join(", ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recipe_with_duplicate_parameter_keys(parameters: &str) -> String {
        format!(
            r#"
version: 1.0.0
title: Duplicate parameters
description: Duplicate parameter validation
instructions: Test {{{{ value }}}}
parameters:
{parameters}
"#
        )
    }

    #[test]
    fn test_rejects_string_then_file_parameter_with_same_key() {
        let recipe_content = recipe_with_duplicate_parameter_keys(
            r#"  - key: value
    input_type: string
    requirement: optional
    default: file.txt
    description: A string parameter
  - key: value
    input_type: file
    requirement: required
    description: A file parameter"#,
        );

        let error = validate_recipe_template_from_content(&recipe_content, None).unwrap_err();

        assert_eq!(error.to_string(), "Duplicate parameter definition: value.");
    }

    #[test]
    fn test_rejects_file_then_string_parameter_with_same_key() {
        let recipe_content = recipe_with_duplicate_parameter_keys(
            r#"  - key: value
    input_type: file
    requirement: required
    description: A file parameter
  - key: value
    input_type: string
    requirement: optional
    default: file.txt
    description: A string parameter"#,
        );

        let error = validate_recipe_template_from_content(&recipe_content, None).unwrap_err();

        assert_eq!(error.to_string(), "Duplicate parameter definition: value.");
    }

    #[test]
    fn test_validate_recipe_template_from_content_success() {
        let recipe_content = r#"
version: 1.0.0
title: Test Recipe
description: A test recipe for validation
instructions: Test instructions with {{ user_role }}
prompt: |
  {% if user_role in ["Director, Account Management", "Senior Director, Account Management"] %}
  - Focus on strategic planning and organizational performance
  {% else %}
  - Provide foundational account management guidance
  {% endif %}
parameters:
  - key: user_role
    input_type: string
    requirement: required
    description: A test parameter
"#;

        let result = validate_recipe_template_from_content(recipe_content, None);
        if let Err(e) = &result {
            eprintln!("Validation error: {}", e);
            eprintln!("Error chain:");
            let mut source = e.source();
            while let Some(err) = source {
                eprintln!("  Caused by: {}", err);
                source = err.source();
            }
        }
        assert!(result.is_ok(), "Validation failed: {:?}", result.err());

        let recipe = result.unwrap();
        assert_eq!(recipe.title, "Test Recipe");
        assert_eq!(recipe.description, "A test recipe for validation");
        assert!(recipe.instructions.is_some());
        println!("Recipe: {:?}", recipe.prompt);
    }

    #[test]
    fn response_json_schema_must_be_an_object() {
        let recipe_content = r#"
version: 1.0.0
title: Boolean schema
description: Boolean schema
instructions: Return structured output
response:
  json_schema: true
"#;

        let error = validate_recipe_template_from_content(recipe_content, None).unwrap_err();

        assert_eq!(error.to_string(), "JSON schema must be an object");
    }

    #[test]
    fn response_json_schema_accepts_an_object_schema() {
        let recipe_content = r#"
version: 1.0.0
title: Object schema
description: Object schema
instructions: Return structured output
response:
  json_schema:
    type: object
    properties:
      result:
        type: string
"#;

        validate_recipe_template_from_content(recipe_content, None).unwrap();
    }

    #[test]
    fn response_json_schema_must_compile() {
        let recipe_content = r#"
version: 1.0.0
title: Invalid pattern
description: Invalid pattern
instructions: Return structured output
response:
  json_schema:
    type: object
    properties:
      result:
        type: string
        pattern: "["
"#;

        let error = validate_recipe_template_from_content(recipe_content, None).unwrap_err();

        assert!(error.to_string().contains("JSON schema validation failed"));
    }
}
