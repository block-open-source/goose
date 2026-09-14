use crate::recipe::read_recipe_file_content::read_parameter_file_content;
use crate::recipe::validate_recipe::validate_recipe_template;
use crate::recipe::{
    Recipe, RecipeParameter, RecipeParameterInputType, RecipeParameterRequirement,
    BUILT_IN_RECIPE_DIR_PARAM,
};
use anyhow::Result;
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum RecipeError {
    #[error("Missing required parameters: {parameters:?}")]
    MissingParams { parameters: Vec<String> },
    #[error("Invalid recipe: {source}")]
    Invalid { source: anyhow::Error },
}

fn render_recipe_template<F>(
    recipe_content: String,
    recipe_dir: &Path,
    params: Vec<(String, String)>,
    user_prompt_fn: Option<F>,
) -> Result<(Option<Recipe>, Vec<String>)>
where
    F: Fn(&str, &str) -> Result<String, anyhow::Error>,
{
    let recipe_dir_str = recipe_dir.display().to_string();

    let validated_recipe = validate_recipe_template(&recipe_content, Some(recipe_dir_str.clone()))?;
    let recipe_parameters = validated_recipe.recipe().parameters.clone();

    let (params_for_template, missing_params) =
        apply_values_to_parameters(&params, recipe_parameters, &recipe_dir_str, user_prompt_fn)?;

    let rendered_recipe = if missing_params.is_empty() {
        Some(validated_recipe.render(&params_for_template)?)
    } else {
        None
    };

    Ok((rendered_recipe, missing_params))
}

pub fn build_recipe_from_template<F>(
    recipe_content: String,
    recipe_dir: &Path,
    params: Vec<(String, String)>,
    user_prompt_fn: Option<F>,
) -> Result<Recipe, RecipeError>
where
    F: Fn(&str, &str) -> Result<String, anyhow::Error>,
{
    let (rendered_recipe, missing_params) =
        render_recipe_template(recipe_content, recipe_dir, params.clone(), user_prompt_fn)
            .map_err(|source| RecipeError::Invalid { source })?;

    let Some(mut recipe) = rendered_recipe else {
        return Err(RecipeError::MissingParams {
            parameters: missing_params,
        });
    };

    if let Some(ref mut sub_recipes) = recipe.sub_recipes {
        for sub_recipe in sub_recipes {
            sub_recipe.path = resolve_sub_recipe_path(&sub_recipe.path, recipe_dir)?;
        }
    }

    Ok(recipe)
}

pub fn apply_values_to_parameters<F>(
    user_params: &[(String, String)],
    recipe_parameters: Option<Vec<RecipeParameter>>,
    recipe_dir: &str,
    user_prompt_fn: Option<F>,
) -> Result<(HashMap<String, String>, Vec<String>)>
where
    F: Fn(&str, &str) -> Result<String, anyhow::Error>,
{
    apply_values_to_parameters_with_file_handler(
        user_params,
        recipe_parameters,
        recipe_dir,
        user_prompt_fn,
        |path| read_parameter_file_content(path),
    )
}

pub fn apply_values_to_parameters_without_file_expansion<F>(
    user_params: &[(String, String)],
    recipe_parameters: Option<Vec<RecipeParameter>>,
    recipe_dir: &str,
    user_prompt_fn: Option<F>,
) -> Result<(HashMap<String, String>, Vec<String>)>
where
    F: Fn(&str, &str) -> Result<String, anyhow::Error>,
{
    apply_values_to_parameters_with_file_handler(
        user_params,
        recipe_parameters,
        recipe_dir,
        user_prompt_fn,
        |path| Ok(path.to_string()),
    )
}

fn apply_values_to_parameters_with_file_handler<F, H>(
    user_params: &[(String, String)],
    recipe_parameters: Option<Vec<RecipeParameter>>,
    recipe_dir: &str,
    user_prompt_fn: Option<F>,
    file_handler: H,
) -> Result<(HashMap<String, String>, Vec<String>)>
where
    F: Fn(&str, &str) -> Result<String, anyhow::Error>,
    H: Fn(&str) -> Result<String, anyhow::Error>,
{
    let mut param_map: HashMap<String, String> = user_params.iter().cloned().collect();
    param_map.insert(
        BUILT_IN_RECIPE_DIR_PARAM.to_string(),
        recipe_dir.to_string(),
    );
    let mut missing_params: Vec<String> = Vec::new();
    for param in recipe_parameters.unwrap_or_default() {
        let raw_value = if let Some(v) = param_map.get(&param.key) {
            Some(v.clone())
        } else {
            match (&param.default, &param.requirement) {
                (Some(default), _) => Some(default.clone()),
                (None, RecipeParameterRequirement::UserPrompt) if user_prompt_fn.is_some() => Some(
                    user_prompt_fn.as_ref().unwrap()(&param.key, &param.description)?,
                ),
                _ => None,
            }
        };

        match raw_value {
            Some(value) => {
                let final_value = if matches!(param.input_type, RecipeParameterInputType::File) {
                    file_handler(&value)?
                } else {
                    value
                };
                param_map.insert(param.key.clone(), final_value);
            }
            None => missing_params.push(param.key.clone()),
        }
    }
    Ok((param_map, missing_params))
}

pub fn resolve_sub_recipe_path(
    sub_recipe_path: &str,
    parent_recipe_dir: &Path,
) -> Result<String, RecipeError> {
    let path = if Path::new(sub_recipe_path).is_absolute() {
        Path::new(sub_recipe_path).to_path_buf()
    } else {
        parent_recipe_dir.join(sub_recipe_path)
    };
    if !path.exists() {
        return Err(RecipeError::Invalid {
            source: anyhow::anyhow!("Sub-recipe file does not exist: {}", path.display()),
        });
    }

    let canonical = path.canonicalize().map_err(|e| RecipeError::Invalid {
        source: anyhow::anyhow!(
            "Failed to resolve sub-recipe path {}: {}",
            path.display(),
            e
        ),
    })?;
    Ok(canonical.display().to_string())
}

#[cfg(test)]
mod tests;
