use anyhow::Result;
use serde_json::Value;
use std::collections::HashMap;
use std::fmt;
use std::path::Path;

use crate::agents::extension::ExtensionConfig;
use crate::agents::types::RetryConfig;
use crate::recipe::read_recipe_file_content::read_recipe_file;
use crate::recipe::yaml_format_utils::reformat_fields_with_multiline_values;
use crate::utils::contains_unicode_tags;
use serde::de::Deserializer;
use serde::{Deserialize, Serialize};

pub mod build_recipe;
pub mod local_recipes;
pub mod manifest;
pub mod read_recipe_file_content;
mod recipe_extension_adapter;
pub mod template_recipe;
pub mod validate_recipe;
pub mod yaml_format_utils;

pub const BUILT_IN_RECIPE_DIR_PARAM: &str = "recipe_dir";
pub const RECIPE_FILE_EXTENSIONS: &[&str] = &["yaml", "json"];

fn default_version() -> String {
    "1.0.0".to_string()
}

/// Strips location information (e.g., "at line X column Y") from error messages
/// to make them more user-friendly for UI display.
pub fn strip_error_location(error_msg: &str) -> String {
    error_msg
        .split(" at line")
        .next()
        .unwrap_or_default()
        .to_string()
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Recipe {
    // Required fields
    #[serde(default = "default_version")]
    pub version: String, // version of the file format, sem ver

    pub title: String, // short title of the recipe

    pub description: String, // a longer description of the recipe

    // Optional fields
    // Note: at least one of instructions or prompt need to be set
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>, // the instructions for the model

    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>, // the prompt to start the session with

    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        deserialize_with = "recipe_extension_adapter::deserialize_recipe_extensions"
    )]
    pub extensions: Option<Vec<ExtensionConfig>>, // a list of extensions to enable

    #[serde(skip_serializing_if = "Option::is_none")]
    pub settings: Option<Settings>, // settings for the recipe

    #[serde(skip_serializing_if = "Option::is_none")]
    pub activities: Option<Vec<String>>, // the activity pills that show up when loading the

    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<Author>, // any additional author information

    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Vec<RecipeParameter>>, // any additional parameters for the recipe

    #[serde(skip_serializing_if = "Option::is_none")]
    pub response: Option<Response>, // response configuration including JSON schema

    #[serde(skip_serializing_if = "Option::is_none")]
    pub sub_recipes: Option<Vec<SubRecipe>>, // sub-recipes for the recipe

    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry: Option<RetryConfig>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Author {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contact: Option<String>, // creator/contact information of the recipe

    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<String>, // any additional metadata for the author
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Settings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub goose_provider: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub goose_model: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<usize>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Response {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub json_schema: Option<serde_json::Value>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SubRecipe {
    pub name: String,
    pub path: String,
    #[serde(default, deserialize_with = "deserialize_value_map_as_string")]
    pub values: Option<HashMap<String, String>>,
    #[serde(default)]
    pub sequential_when_repeated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

fn deserialize_value_map_as_string<'de, D>(
    deserializer: D,
) -> Result<Option<HashMap<String, String>>, D::Error>
where
    D: Deserializer<'de>,
{
    // First, try to deserialize a map of values
    let opt_raw: Option<HashMap<String, Value>> = Option::deserialize(deserializer)?;

    match opt_raw {
        Some(raw_map) => {
            let mut result = HashMap::new();
            for (k, v) in raw_map {
                let s = match v {
                    Value::String(s) => s,
                    _ => serde_json::to_string(&v).map_err(serde::de::Error::custom)?,
                };
                result.insert(k, s);
            }
            Ok(Some(result))
        }
        None => Ok(None),
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
pub enum RecipeParameterRequirement {
    Required,
    Optional,
    UserPrompt,
}

impl fmt::Display for RecipeParameterRequirement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            serde_json::to_string(self).unwrap().trim_matches('"')
        )
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
pub enum RecipeParameterInputType {
    String,
    Number,
    Boolean,
    Date,
    /// File parameter that imports content from a file path.
    /// Cannot have default values to prevent importing sensitive user files.
    File,
    Select,
}

impl fmt::Display for RecipeParameterInputType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            serde_json::to_string(self).unwrap().trim_matches('"')
        )
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RecipeParameter {
    pub key: String,
    pub input_type: RecipeParameterInputType,
    pub requirement: RecipeParameterRequirement,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<String>>,
}

/// Builder for creating Recipe instances
pub struct RecipeBuilder {
    // Required fields with default values
    version: String,
    title: Option<String>,
    description: Option<String>,
    instructions: Option<String>,

    // Optional fields
    prompt: Option<String>,
    extensions: Option<Vec<ExtensionConfig>>,
    settings: Option<Settings>,
    activities: Option<Vec<String>>,
    author: Option<Author>,
    parameters: Option<Vec<RecipeParameter>>,
    response: Option<Response>,
    sub_recipes: Option<Vec<SubRecipe>>,
    retry: Option<RetryConfig>,
}

impl Recipe {
    /// When a recipe has the old builtin developer extension but not analyze, auto-inject analyze.
    fn ensure_analyze_for_developer(&mut self) {
        let has_builtin_developer = self.extensions.as_ref().is_some_and(|exts| {
            exts.iter()
                .any(|e| matches!(e, ExtensionConfig::Builtin { name, .. } if name == "developer"))
        });
        let has_analyze = self
            .extensions
            .as_ref()
            .is_some_and(|exts| exts.iter().any(|e| e.name() == "analyze"));

        if has_builtin_developer && !has_analyze {
            let analyze = ExtensionConfig::Platform {
                name: "analyze".to_string(),
                description: String::new(),
                display_name: None,
                bundled: None,
                available_tools: vec![],
            };
            if let Some(exts) = &mut self.extensions {
                exts.push(analyze);
            }
        }
    }

    fn ensure_summon_for_subrecipes(&mut self) {
        if self.sub_recipes.is_none() {
            return;
        }
        let summon = ExtensionConfig::Platform {
            name: "summon".to_string(),
            description: String::new(),
            display_name: None,
            bundled: None,
            available_tools: vec![],
        };
        match &mut self.extensions {
            Some(exts) if !exts.iter().any(|e| e.name() == "summon") => exts.push(summon),
            None => self.extensions = Some(vec![summon]),
            _ => {}
        }
    }

    /// Returns true if harmful content is detected in instructions, prompt, or activities fields
    pub fn check_for_security_warnings(&self) -> bool {
        if [self.instructions.as_deref(), self.prompt.as_deref()]
            .iter()
            .flatten()
            .any(|&field| contains_unicode_tags(field))
        {
            return true;
        }

        if let Some(activities) = &self.activities {
            return activities
                .iter()
                .any(|activity| contains_unicode_tags(activity));
        }

        false
    }

    pub fn to_yaml(&self) -> Result<String> {
        let recipe_yaml = serde_yaml::to_string(self)
            .map_err(|err| anyhow::anyhow!("Failed to serialize recipe: {}", err))?;
        let formatted_recipe_yaml =
            reformat_fields_with_multiline_values(&recipe_yaml, &["prompt", "instructions"]);
        Ok(formatted_recipe_yaml)
    }

    pub fn builder() -> RecipeBuilder {
        RecipeBuilder {
            version: default_version(),
            title: None,
            description: None,
            instructions: None,
            prompt: None,
            extensions: None,
            settings: None,
            activities: None,
            author: None,
            parameters: None,
            response: None,
            sub_recipes: None,
            retry: None,
        }
    }

    pub fn from_file_path(file_path: &Path) -> Result<Self> {
        let file = read_recipe_file(file_path)?;
        Self::from_content(&file.content)
    }

    pub fn from_content(content: &str) -> Result<Self> {
        let mut recipe: Recipe = match serde_yaml::from_str::<serde_yaml::Value>(content) {
            Ok(yaml_value) => {
                if let Some(nested_recipe) = yaml_value.get("recipe") {
                    serde_yaml::from_value(nested_recipe.clone())
                        .map_err(|e| anyhow::anyhow!("{}", strip_error_location(&e.to_string())))?
                } else {
                    serde_yaml::from_str(content)
                        .map_err(|e| anyhow::anyhow!("{}", strip_error_location(&e.to_string())))?
                }
            }
            Err(_) => serde_yaml::from_str(content)
                .map_err(|e| anyhow::anyhow!("{}", strip_error_location(&e.to_string())))?,
        };

        recipe.ensure_analyze_for_developer();
        recipe.ensure_summon_for_subrecipes();
        Ok(recipe)
    }
}

impl RecipeBuilder {
    pub fn version(mut self, version: impl Into<String>) -> Self {
        self.version = version.into();
        self
    }

    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn instructions(mut self, instructions: impl Into<String>) -> Self {
        self.instructions = Some(instructions.into());
        self
    }

    pub fn prompt(mut self, prompt: impl Into<String>) -> Self {
        self.prompt = Some(prompt.into());
        self
    }

    pub fn extensions(mut self, extensions: Vec<ExtensionConfig>) -> Self {
        self.extensions = Some(extensions);
        self
    }

    pub fn settings(mut self, settings: Settings) -> Self {
        self.settings = Some(settings);
        self
    }

    pub fn activities(mut self, activities: Vec<String>) -> Self {
        self.activities = Some(activities);
        self
    }

    pub fn author(mut self, author: Author) -> Self {
        self.author = Some(author);
        self
    }

    pub fn parameters(mut self, parameters: Vec<RecipeParameter>) -> Self {
        self.parameters = Some(parameters);
        self
    }

    pub fn response(mut self, response: Response) -> Self {
        self.response = Some(response);
        self
    }

    pub fn sub_recipes(mut self, sub_recipes: Vec<SubRecipe>) -> Self {
        self.sub_recipes = Some(sub_recipes);
        self
    }

    pub fn retry(mut self, retry: RetryConfig) -> Self {
        self.retry = Some(retry);
        self
    }

    pub fn build(self) -> Result<Recipe, &'static str> {
        let title = self.title.ok_or("Title is required")?;
        let description = self.description.ok_or("Description is required")?;

        if self.instructions.is_none() && self.prompt.is_none() {
            return Err("At least one of 'prompt' or 'instructions' is required");
        }

        Ok(Recipe {
            version: self.version,
            title,
            description,
            instructions: self.instructions,
            prompt: self.prompt,
            extensions: self.extensions,
            settings: self.settings,
            activities: self.activities,
            author: self.author,
            parameters: self.parameters,
            response: self.response,
            sub_recipes: self.sub_recipes,
            retry: self.retry,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_content_with_json() {
        let content = r#"{
            "version": "1.0.0",
            "title": "Test Recipe",
            "description": "A test recipe",
            "prompt": "Test prompt",
            "instructions": "Test instructions",
            "extensions": [
                {
                    "type": "stdio",
                    "name": "test_extension",
                    "cmd": "test_cmd",
                    "args": ["arg1", "arg2"],
                    "timeout": 300,
                    "description": "Test extension"
                }
            ],
            "parameters": [
                {
                    "key": "test_param",
                    "input_type": "string",
                    "requirement": "required",
                    "description": "A test parameter"
                }
            ],
            "response": {
                "json_schema": {
                    "type": "object",
                    "properties": {
                        "name": {
                            "type": "string"
                        },
                        "age": {
                            "type": "number"
                        }
                    },
                    "required": ["name"]
                }
            },
            "sub_recipes": [
                {
                    "name": "test_sub_recipe",
                    "path": "test_sub_recipe.yaml",
                    "values": {
                        "sub_recipe_param": "sub_recipe_value"
                    }
                }
            ]
        }"#;

        let recipe = Recipe::from_content(content).unwrap();
        assert_eq!(recipe.version, "1.0.0");
        assert_eq!(recipe.title, "Test Recipe");
        assert_eq!(recipe.description, "A test recipe");
        assert_eq!(recipe.instructions, Some("Test instructions".to_string()));
        assert_eq!(recipe.prompt, Some("Test prompt".to_string()));

        assert!(recipe.extensions.is_some());
        let extensions = recipe.extensions.as_ref().unwrap();
        assert_eq!(extensions.len(), 2);
        assert!(extensions.iter().any(|e| e.name() == "test_extension"));
        assert!(extensions.iter().any(|e| e.name() == "summon"));

        assert!(recipe.parameters.is_some());
        let parameters = recipe.parameters.unwrap();
        assert_eq!(parameters.len(), 1);
        assert_eq!(parameters[0].key, "test_param");
        assert!(matches!(
            parameters[0].input_type,
            RecipeParameterInputType::String
        ));
        assert!(matches!(
            parameters[0].requirement,
            RecipeParameterRequirement::Required
        ));

        assert!(recipe.response.is_some());
        let response = recipe.response.unwrap();
        assert!(response.json_schema.is_some());
        let json_schema = response.json_schema.unwrap();
        assert_eq!(json_schema["type"], "object");
        assert!(json_schema["properties"].is_object());
        assert_eq!(json_schema["properties"]["name"]["type"], "string");
        assert_eq!(json_schema["properties"]["age"]["type"], "number");
        assert_eq!(json_schema["required"], serde_json::json!(["name"]));

        assert!(recipe.sub_recipes.is_some());
        let sub_recipes = recipe.sub_recipes.unwrap();
        assert_eq!(sub_recipes.len(), 1);
        assert_eq!(sub_recipes[0].name, "test_sub_recipe");
        assert_eq!(sub_recipes[0].path, "test_sub_recipe.yaml");
        assert_eq!(
            sub_recipes[0].values,
            Some(HashMap::from([(
                "sub_recipe_param".to_string(),
                "sub_recipe_value".to_string()
            )]))
        );
    }

    #[test]
    fn test_from_content_with_yaml() {
        let content = r#"version: 1.0.0
title: Test Recipe
description: A test recipe
prompt: Test prompt
instructions: Test instructions
extensions:
  - type: stdio
    name: test_extension
    cmd: test_cmd
    args: [arg1, arg2]
    timeout: 300
    description: Test extension
parameters:
  - key: test_param
    input_type: string
    requirement: required
    description: A test parameter
response:
  json_schema:
    type: object
    properties:
      name:
        type: string
      age:
        type: number
    required:
      - name
sub_recipes:
  - name: test_sub_recipe
    path: test_sub_recipe.yaml
    values:
      sub_recipe_param: sub_recipe_value"#;

        let recipe = Recipe::from_content(content).unwrap();
        assert_eq!(recipe.version, "1.0.0");
        assert_eq!(recipe.title, "Test Recipe");
        assert_eq!(recipe.description, "A test recipe");
        assert_eq!(recipe.instructions, Some("Test instructions".to_string()));
        assert_eq!(recipe.prompt, Some("Test prompt".to_string()));

        assert!(recipe.extensions.is_some());
        let extensions = recipe.extensions.as_ref().unwrap();
        assert_eq!(extensions.len(), 2);
        assert!(extensions.iter().any(|e| e.name() == "test_extension"));
        assert!(extensions.iter().any(|e| e.name() == "summon"));

        assert!(recipe.parameters.is_some());
        let parameters = recipe.parameters.unwrap();
        assert_eq!(parameters.len(), 1);
        assert_eq!(parameters[0].key, "test_param");
        assert!(matches!(
            parameters[0].input_type,
            RecipeParameterInputType::String
        ));
        assert!(matches!(
            parameters[0].requirement,
            RecipeParameterRequirement::Required
        ));

        assert!(recipe.response.is_some());
        let response = recipe.response.unwrap();
        assert!(response.json_schema.is_some());
        let json_schema = response.json_schema.unwrap();
        assert_eq!(json_schema["type"], "object");
        assert!(json_schema["properties"].is_object());
        assert_eq!(json_schema["properties"]["name"]["type"], "string");
        assert_eq!(json_schema["properties"]["age"]["type"], "number");
        assert_eq!(json_schema["required"], serde_json::json!(["name"]));

        assert!(recipe.sub_recipes.is_some());
        let sub_recipes = recipe.sub_recipes.unwrap();
        assert_eq!(sub_recipes.len(), 1);
        assert_eq!(sub_recipes[0].name, "test_sub_recipe");
        assert_eq!(sub_recipes[0].path, "test_sub_recipe.yaml");
        assert_eq!(
            sub_recipes[0].values,
            Some(HashMap::from([(
                "sub_recipe_param".to_string(),
                "sub_recipe_value".to_string()
            )]))
        );
    }

    #[test]
    fn test_from_content_invalid_json() {
        let content = "{ invalid json }";

        let result = Recipe::from_content(content);
        assert!(result.is_err());
    }

    #[test]
    fn test_from_content_missing_required_fields() {
        let content = r#"{
            "version": "1.0.0",
            "description": "A test recipe"
        }"#;

        let result = Recipe::from_content(content);
        assert!(result.is_err());
    }

    #[test]
    fn test_from_content_with_author() {
        let content = r#"{
            "version": "1.0.0",
            "title": "Test Recipe",
            "description": "A test recipe",
            "instructions": "Test instructions",
            "author": {
                "contact": "test@example.com"
            }
        }"#;

        let recipe = Recipe::from_content(content).unwrap();

        assert!(recipe.author.is_some());
        let author = recipe.author.unwrap();
        assert_eq!(author.contact, Some("test@example.com".to_string()));
    }

    #[test]
    fn test_from_content_with_activities() {
        let content = r#"{
            "version": "1.0.0",
            "title": "Test Recipe",
            "description": "A test recipe",
            "instructions": "Test instructions",
            "activities": ["activity1", "activity2"]
        }"#;

        let recipe = Recipe::from_content(content).unwrap();

        assert!(recipe.activities.is_some());
        let activities = recipe.activities.unwrap();
        assert_eq!(activities, vec!["activity1", "activity2"]);
    }

    #[test]
    fn test_from_content_with_nested_recipe_yaml() {
        let content = r#"name: test_recipe
recipe:
  title: Nested Recipe Test
  description: A test recipe with nested structure
  instructions: Test instructions for nested recipe
  activities:
    - Test activity 1
    - Test activity 2
  prompt: Test prompt
  extensions: []
isGlobal: true"#;

        let recipe = Recipe::from_content(content).unwrap();
        assert_eq!(recipe.title, "Nested Recipe Test");
        assert_eq!(recipe.description, "A test recipe with nested structure");
        assert_eq!(
            recipe.instructions,
            Some("Test instructions for nested recipe".to_string())
        );
        assert_eq!(recipe.prompt, Some("Test prompt".to_string()));
        assert!(recipe.activities.is_some());
        let activities = recipe.activities.unwrap();
        assert_eq!(activities, vec!["Test activity 1", "Test activity 2"]);
        assert!(recipe.extensions.is_some());
        let extensions = recipe.extensions.unwrap();
        assert_eq!(extensions.len(), 0);
    }

    #[test]
    fn test_check_for_security_warnings() {
        let mut recipe = Recipe {
            version: "1.0.0".to_string(),
            title: "Test".to_string(),
            description: "Test".to_string(),
            instructions: Some("clean instructions".to_string()),
            prompt: Some("clean prompt".to_string()),
            extensions: None,
            settings: None,
            activities: Some(vec!["clean activity 1".to_string()]),
            author: None,
            parameters: None,
            response: None,
            sub_recipes: None,
            retry: None,
        };

        assert!(!recipe.check_for_security_warnings());

        // Malicious activities
        recipe.activities = Some(vec![
            "clean activity".to_string(),
            format!("malicious{}activity", '\u{E0041}'),
        ]);
        assert!(recipe.check_for_security_warnings());

        // Malicious instructions
        recipe.instructions = Some(format!("instructions{}", '\u{E0041}'));
        assert!(recipe.check_for_security_warnings());

        // Malicious prompt
        recipe.prompt = Some(format!("prompt{}", '\u{E0042}'));
        assert!(recipe.check_for_security_warnings());
    }

    #[test]
    fn test_from_content_with_null_description() {
        let content = r#"{
            "version": "1.0.0",
            "title": "Test Recipe",
            "description": "A test recipe",
            "instructions": "Test instructions",
            "extensions": [
                {
                    "type": "stdio",
                    "name": "test_extension",
                    "cmd": "test_cmd",
                    "args": [],
                    "timeout": 300,
                    "description": null
                }
            ]
        }"#;

        let recipe = Recipe::from_content(content).unwrap();

        assert!(recipe.extensions.is_some());
        let extensions = recipe.extensions.unwrap();
        assert_eq!(extensions.len(), 1);

        if let ExtensionConfig::Stdio {
            name, description, ..
        } = &extensions[0]
        {
            assert_eq!(name, "test_extension");
            assert_eq!(description, "");
        } else {
            panic!("Expected Stdio extension");
        }
    }

    #[test]
    fn test_format_serde_error_removes_location() {
        let content = r#"{"version": "1.0.0"}"#;

        let result = Recipe::from_content(content);
        assert!(result.is_err());

        let error_msg = result.unwrap_err().to_string();
        assert_eq!(error_msg, "missing field `title`");
    }

    #[test]
    fn test_format_serde_error_missing_title() {
        let content = r#"{
            "version": "1.0.0",
            "description": "A test recipe",
            "instructions": "Test instructions"
        }"#;

        let result = Recipe::from_content(content);
        assert!(result.is_err());

        let error_msg = result.unwrap_err().to_string();
        assert_eq!(error_msg, "missing field `title`");
    }

    #[test]
    fn test_format_serde_error_invalid_type() {
        let content = r#"{
            "version": "1.0.0",
            "title": "Test",
            "description": "Test",
            "instructions": "Test",
            "settings": {
                "temperature": "not_a_number"
            }
        }"#;

        let result = Recipe::from_content(content);
        assert!(result.is_err());

        let error_msg = result.unwrap_err().to_string();
        assert_eq!(
            error_msg,
            "settings.temperature: invalid type: string \"not_a_number\", expected f32"
        );
    }
}
