use anyhow::Result;
use goose_sdk_types::custom_requests::{
    RecipeAuthorDto, RecipeDto, RecipeExtensionDto, RecipeParameterDto,
    RecipeParameterInputTypeDto, RecipeParameterRequirementDto, RecipeResponseDto,
    RecipeRetryConfigDto, RecipeSettingsDto, RecipeSuccessCheckDto, SubRecipeDto,
};

use crate::agents::extension::{Envs, ExtensionConfig};
use crate::agents::types::{RetryConfig, SuccessCheck};
use crate::recipe::{
    Author, Recipe, RecipeParameter, RecipeParameterInputType, RecipeParameterRequirement,
    Response, Settings, SubRecipe,
};

impl TryFrom<RecipeDto> for Recipe {
    type Error = anyhow::Error;

    fn try_from(dto: RecipeDto) -> Result<Self> {
        Ok(Self {
            version: dto.version,
            title: dto.title,
            description: dto.description,
            instructions: dto.instructions,
            prompt: dto.prompt,
            extensions: dto
                .extensions
                .map(|extensions| {
                    extensions
                        .into_iter()
                        .map(ExtensionConfig::try_from)
                        .collect::<Result<Vec<_>>>()
                })
                .transpose()?,
            settings: dto.settings.map(Settings::from),
            activities: dto.activities,
            author: dto.author.map(Author::from),
            parameters: dto
                .parameters
                .map(|parameters| parameters.into_iter().map(RecipeParameter::from).collect()),
            response: dto.response.map(Response::from),
            sub_recipes: dto
                .sub_recipes
                .map(|sub_recipes| sub_recipes.into_iter().map(SubRecipe::from).collect()),
            retry: dto.retry.map(RetryConfig::from),
        })
    }
}

impl TryFrom<Recipe> for RecipeDto {
    type Error = anyhow::Error;

    fn try_from(recipe: Recipe) -> Result<Self> {
        Ok(Self {
            version: recipe.version,
            title: recipe.title,
            description: recipe.description,
            instructions: recipe.instructions,
            prompt: recipe.prompt,
            extensions: recipe
                .extensions
                .map(|extensions| {
                    extensions
                        .into_iter()
                        .map(RecipeExtensionDto::try_from)
                        .collect::<Result<Vec<_>>>()
                })
                .transpose()?,
            settings: recipe.settings.map(RecipeSettingsDto::from),
            activities: recipe.activities,
            author: recipe.author.map(RecipeAuthorDto::from),
            parameters: recipe.parameters.map(|parameters| {
                parameters
                    .into_iter()
                    .map(RecipeParameterDto::from)
                    .collect()
            }),
            response: recipe.response.map(RecipeResponseDto::from),
            sub_recipes: recipe
                .sub_recipes
                .map(|sub_recipes| sub_recipes.into_iter().map(SubRecipeDto::from).collect()),
            retry: recipe.retry.map(RecipeRetryConfigDto::from),
        })
    }
}

impl From<RecipeAuthorDto> for Author {
    fn from(dto: RecipeAuthorDto) -> Self {
        Self {
            contact: dto.contact,
            metadata: dto.metadata,
        }
    }
}

impl From<Author> for RecipeAuthorDto {
    fn from(author: Author) -> Self {
        Self {
            contact: author.contact,
            metadata: author.metadata,
        }
    }
}

impl From<RecipeSettingsDto> for Settings {
    fn from(dto: RecipeSettingsDto) -> Self {
        Self {
            goose_provider: dto.goose_provider,
            goose_model: dto.goose_model,
            temperature: dto.temperature,
            max_turns: dto.max_turns,
        }
    }
}

impl From<Settings> for RecipeSettingsDto {
    fn from(settings: Settings) -> Self {
        Self {
            goose_provider: settings.goose_provider,
            goose_model: settings.goose_model,
            temperature: settings.temperature,
            max_turns: settings.max_turns,
        }
    }
}

impl From<RecipeResponseDto> for Response {
    fn from(dto: RecipeResponseDto) -> Self {
        Self {
            json_schema: dto.json_schema,
        }
    }
}

impl From<Response> for RecipeResponseDto {
    fn from(response: Response) -> Self {
        Self {
            json_schema: response.json_schema,
        }
    }
}

impl From<SubRecipeDto> for SubRecipe {
    fn from(dto: SubRecipeDto) -> Self {
        Self {
            name: dto.name,
            path: dto.path,
            values: dto.values,
            sequential_when_repeated: dto.sequential_when_repeated,
            description: dto.description,
        }
    }
}

impl From<SubRecipe> for SubRecipeDto {
    fn from(sub_recipe: SubRecipe) -> Self {
        Self {
            name: sub_recipe.name,
            path: sub_recipe.path,
            values: sub_recipe.values,
            sequential_when_repeated: sub_recipe.sequential_when_repeated,
            description: sub_recipe.description,
        }
    }
}

impl From<RecipeParameterDto> for RecipeParameter {
    fn from(dto: RecipeParameterDto) -> Self {
        Self {
            key: dto.key,
            input_type: RecipeParameterInputType::from(dto.input_type),
            requirement: RecipeParameterRequirement::from(dto.requirement),
            description: dto.description,
            default: dto.default,
            options: dto.options,
        }
    }
}

impl From<RecipeParameter> for RecipeParameterDto {
    fn from(parameter: RecipeParameter) -> Self {
        Self {
            key: parameter.key,
            input_type: RecipeParameterInputTypeDto::from(parameter.input_type),
            requirement: RecipeParameterRequirementDto::from(parameter.requirement),
            description: parameter.description,
            default: parameter.default,
            options: parameter.options,
        }
    }
}

impl From<RecipeParameterInputTypeDto> for RecipeParameterInputType {
    fn from(dto: RecipeParameterInputTypeDto) -> Self {
        match dto {
            RecipeParameterInputTypeDto::String => Self::String,
            RecipeParameterInputTypeDto::Number => Self::Number,
            RecipeParameterInputTypeDto::Boolean => Self::Boolean,
            RecipeParameterInputTypeDto::Date => Self::Date,
            RecipeParameterInputTypeDto::File => Self::File,
            RecipeParameterInputTypeDto::Select => Self::Select,
        }
    }
}

impl From<RecipeParameterInputType> for RecipeParameterInputTypeDto {
    fn from(input_type: RecipeParameterInputType) -> Self {
        match input_type {
            RecipeParameterInputType::String => Self::String,
            RecipeParameterInputType::Number => Self::Number,
            RecipeParameterInputType::Boolean => Self::Boolean,
            RecipeParameterInputType::Date => Self::Date,
            RecipeParameterInputType::File => Self::File,
            RecipeParameterInputType::Select => Self::Select,
        }
    }
}

impl From<RecipeParameterRequirementDto> for RecipeParameterRequirement {
    fn from(dto: RecipeParameterRequirementDto) -> Self {
        match dto {
            RecipeParameterRequirementDto::Required => Self::Required,
            RecipeParameterRequirementDto::Optional => Self::Optional,
            RecipeParameterRequirementDto::UserPrompt => Self::UserPrompt,
        }
    }
}

impl From<RecipeParameterRequirement> for RecipeParameterRequirementDto {
    fn from(requirement: RecipeParameterRequirement) -> Self {
        match requirement {
            RecipeParameterRequirement::Required => Self::Required,
            RecipeParameterRequirement::Optional => Self::Optional,
            RecipeParameterRequirement::UserPrompt => Self::UserPrompt,
        }
    }
}

impl From<RecipeRetryConfigDto> for RetryConfig {
    fn from(dto: RecipeRetryConfigDto) -> Self {
        Self {
            max_retries: dto.max_retries,
            checks: dto.checks.into_iter().map(SuccessCheck::from).collect(),
            on_failure: dto.on_failure,
            timeout_seconds: dto.timeout_seconds,
            on_failure_timeout_seconds: dto.on_failure_timeout_seconds,
        }
    }
}

impl From<RetryConfig> for RecipeRetryConfigDto {
    fn from(retry: RetryConfig) -> Self {
        Self {
            max_retries: retry.max_retries,
            checks: retry
                .checks
                .into_iter()
                .map(RecipeSuccessCheckDto::from)
                .collect(),
            on_failure: retry.on_failure,
            timeout_seconds: retry.timeout_seconds,
            on_failure_timeout_seconds: retry.on_failure_timeout_seconds,
        }
    }
}

impl From<RecipeSuccessCheckDto> for SuccessCheck {
    fn from(dto: RecipeSuccessCheckDto) -> Self {
        match dto {
            RecipeSuccessCheckDto::Shell { command } => Self::Shell { command },
        }
    }
}

impl From<SuccessCheck> for RecipeSuccessCheckDto {
    fn from(check: SuccessCheck) -> Self {
        match check {
            SuccessCheck::Shell { command } => Self::Shell { command },
        }
    }
}

impl TryFrom<RecipeExtensionDto> for ExtensionConfig {
    type Error = anyhow::Error;

    fn try_from(dto: RecipeExtensionDto) -> Result<Self> {
        Ok(match dto {
            RecipeExtensionDto::Builtin {
                name,
                description,
                display_name,
                timeout,
                bundled,
                available_tools,
            } => Self::Builtin {
                name,
                description: description.unwrap_or_default(),
                display_name,
                timeout,
                bundled,
                available_tools: available_tools.unwrap_or_default(),
            },
            RecipeExtensionDto::Platform {
                name,
                description,
                display_name,
                bundled,
                available_tools,
            } => Self::Platform {
                name,
                description: description.unwrap_or_default(),
                display_name,
                bundled,
                available_tools: available_tools.unwrap_or_default(),
            },
            RecipeExtensionDto::Stdio {
                name,
                description,
                cmd,
                args,
                envs,
                env_keys,
                timeout,
                cwd,
                bundled,
                available_tools,
            } => Self::Stdio {
                name,
                description: description.unwrap_or_default(),
                cmd,
                args,
                envs: Envs::new(envs),
                env_keys,
                timeout,
                cwd,
                bundled,
                available_tools: available_tools.unwrap_or_default(),
            },
            RecipeExtensionDto::StreamableHttp {
                name,
                description,
                uri,
                envs,
                env_keys,
                headers,
                timeout,
                socket,
                client_id,
                client_secret_key,
                scopes,
                bundled,
                available_tools,
            } => Self::StreamableHttp {
                name,
                description: description.unwrap_or_default(),
                uri,
                envs: Envs::new(envs),
                env_keys,
                headers,
                timeout,
                socket,
                client_id,
                client_secret_key,
                scopes,
                bundled,
                available_tools: available_tools.unwrap_or_default(),
            },
        })
    }
}

impl TryFrom<ExtensionConfig> for RecipeExtensionDto {
    type Error = anyhow::Error;

    fn try_from(extension: ExtensionConfig) -> Result<Self> {
        Ok(match extension {
            ExtensionConfig::Builtin {
                name,
                description,
                display_name,
                timeout,
                bundled,
                available_tools,
            } => Self::Builtin {
                name,
                description: Some(description),
                display_name,
                timeout,
                bundled,
                available_tools: available_tools_to_wire(available_tools),
            },
            ExtensionConfig::Platform {
                name,
                description,
                display_name,
                bundled,
                available_tools,
            } => Self::Platform {
                name,
                description: Some(description),
                display_name,
                bundled,
                available_tools: available_tools_to_wire(available_tools),
            },
            ExtensionConfig::Stdio {
                name,
                description,
                cmd,
                args,
                envs,
                env_keys,
                timeout,
                cwd,
                bundled,
                available_tools,
            } => Self::Stdio {
                name,
                description: Some(description),
                cmd,
                args,
                envs: envs.get_env(),
                env_keys,
                timeout,
                cwd,
                bundled,
                available_tools: available_tools_to_wire(available_tools),
            },
            ExtensionConfig::StreamableHttp {
                name,
                description,
                uri,
                envs,
                env_keys,
                headers,
                timeout,
                socket,
                client_id,
                client_secret_key,
                scopes,
                bundled,
                available_tools,
            } => Self::StreamableHttp {
                name,
                description: Some(description),
                uri,
                envs: envs.get_env(),
                env_keys,
                headers,
                timeout,
                socket,
                client_id,
                client_secret_key,
                scopes,
                bundled,
                available_tools: available_tools_to_wire(available_tools),
            },
        })
    }
}

fn available_tools_to_wire(available_tools: Vec<String>) -> Option<Vec<String>> {
    if available_tools.is_empty() {
        None
    } else {
        Some(available_tools)
    }
}

pub fn recipe_manifest_to_list_entry_dto(
    id: String,
    recipe: Recipe,
    file_path: impl ToString,
    last_modified: String,
    schedule_cron: Option<String>,
    slash_command: Option<String>,
) -> Result<goose_sdk_types::custom_requests::RecipeListEntryDto> {
    Ok(goose_sdk_types::custom_requests::RecipeListEntryDto {
        id,
        recipe: RecipeDto::try_from(recipe)?,
        file_path: file_path.to_string(),
        last_modified,
        schedule_cron,
        slash_command,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::json;

    use super::*;

    #[test]
    fn converts_recipe_dto_to_recipe_and_back() {
        let dto = RecipeDto {
            title: "Test Recipe".to_string(),
            description: "A recipe used by conversion tests".to_string(),
            instructions: Some("Follow the instructions".to_string()),
            prompt: Some("Start here".to_string()),
            extensions: Some(vec![
                RecipeExtensionDto::Builtin {
                    name: "developer".to_string(),
                    description: Some("Developer tools".to_string()),
                    display_name: Some("Developer".to_string()),
                    timeout: Some(300),
                    bundled: Some(true),
                    available_tools: Some(vec!["shell".to_string()]),
                },
                RecipeExtensionDto::Stdio {
                    name: "local".to_string(),
                    description: Some("Local tool".to_string()),
                    cmd: "goose-mcp".to_string(),
                    args: vec!["run".to_string()],
                    envs: HashMap::from([("LOCAL_MODE".to_string(), "true".to_string())]),
                    env_keys: vec!["API_KEY".to_string()],
                    timeout: Some(60),
                    cwd: Some("/tmp".to_string()),
                    bundled: None,
                    available_tools: Some(vec!["run".to_string()]),
                },
                RecipeExtensionDto::StreamableHttp {
                    name: "remote".to_string(),
                    description: Some("Remote tool".to_string()),
                    uri: "http://localhost:3000/mcp".to_string(),
                    envs: HashMap::from([("REMOTE_MODE".to_string(), "true".to_string())]),
                    env_keys: vec!["TOKEN".to_string()],
                    headers: HashMap::from([("X-Test".to_string(), "true".to_string())]),
                    timeout: Some(30),
                    socket: None,
                    client_id: None,
                    client_secret_key: None,
                    scopes: vec![],
                    bundled: Some(false),
                    available_tools: Some(vec!["fetch".to_string()]),
                },
            ]),
            settings: Some(RecipeSettingsDto {
                goose_provider: Some("openai".to_string()),
                goose_model: Some("gpt-5".to_string()),
                temperature: Some(0.2),
                max_turns: Some(4),
            }),
            activities: Some(vec!["plan".to_string(), "build".to_string()]),
            author: Some(RecipeAuthorDto {
                contact: Some("test@example.com".to_string()),
                metadata: Some("metadata".to_string()),
            }),
            parameters: Some(vec![RecipeParameterDto {
                key: "environment".to_string(),
                input_type: RecipeParameterInputTypeDto::Select,
                requirement: RecipeParameterRequirementDto::Required,
                description: "Target environment".to_string(),
                default: Some("dev".to_string()),
                options: Some(vec!["dev".to_string(), "prod".to_string()]),
            }]),
            response: Some(RecipeResponseDto {
                json_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "ok": { "type": "boolean" }
                    }
                })),
            }),
            sub_recipes: Some(vec![SubRecipeDto {
                name: "child".to_string(),
                path: "child.yaml".to_string(),
                values: Some(HashMap::from([("target".to_string(), "dev".to_string())])),
                sequential_when_repeated: true,
                description: Some("Child recipe".to_string()),
            }]),
            retry: Some(RecipeRetryConfigDto {
                max_retries: 2,
                checks: vec![RecipeSuccessCheckDto::Shell {
                    command: "test -f output.json".to_string(),
                }],
                on_failure: Some("rm -f output.json".to_string()),
                timeout_seconds: Some(10),
                on_failure_timeout_seconds: Some(20),
            }),
            ..RecipeDto::default()
        };

        let recipe = Recipe::try_from(dto).unwrap();
        assert_eq!(recipe.version, "1.0.0");
        assert_eq!(recipe.title, "Test Recipe");
        assert_eq!(recipe.extensions.as_ref().unwrap().len(), 3);
        match &recipe.extensions.as_ref().unwrap()[0] {
            ExtensionConfig::Builtin {
                available_tools, ..
            } => {
                assert_eq!(available_tools, &vec!["shell".to_string()]);
            }
            extension => panic!("expected builtin extension, got {extension:?}"),
        }
        match &recipe.extensions.as_ref().unwrap()[1] {
            ExtensionConfig::Stdio {
                envs,
                available_tools,
                ..
            } => {
                assert_eq!(envs.get_env()["LOCAL_MODE"], "true");
                assert_eq!(available_tools, &vec!["run".to_string()]);
            }
            extension => panic!("expected stdio extension, got {extension:?}"),
        }
        match &recipe.extensions.as_ref().unwrap()[2] {
            ExtensionConfig::StreamableHttp {
                envs,
                available_tools,
                ..
            } => {
                assert_eq!(envs.get_env()["REMOTE_MODE"], "true");
                assert_eq!(available_tools, &vec!["fetch".to_string()]);
            }
            extension => panic!("expected streamable_http extension, got {extension:?}"),
        }
        assert_eq!(
            recipe.sub_recipes.as_ref().unwrap()[0].values,
            Some(HashMap::from([("target".to_string(), "dev".to_string())]))
        );
        assert_eq!(recipe.retry.as_ref().unwrap().max_retries, 2);

        let round_tripped = RecipeDto::try_from(recipe).unwrap();
        let serialized = serde_json::to_value(round_tripped).unwrap();
        assert!(serialized.get("sub_recipes").is_some());
        assert!(serialized.get("subRecipes").is_none());
        assert_eq!(serialized["parameters"][0]["input_type"], json!("select"));
        assert_eq!(serialized["retry"]["checks"][0]["type"], json!("shell"));
        assert_eq!(serialized["extensions"][0]["available_tools"][0], "shell");
        assert_eq!(serialized["extensions"][1]["envs"]["LOCAL_MODE"], "true");
        assert_eq!(serialized["extensions"][1]["available_tools"][0], "run");
        assert_eq!(serialized["extensions"][2]["envs"]["REMOTE_MODE"], "true");
        assert_eq!(serialized["extensions"][2]["available_tools"][0], "fetch");
    }
}
