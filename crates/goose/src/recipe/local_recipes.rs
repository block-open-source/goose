use anyhow::{anyhow, Result};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::paths::Paths;
use crate::recipe::read_recipe_file_content::{read_recipe_file, RecipeFile};
use crate::recipe::Recipe;
use crate::recipe::RECIPE_FILE_EXTENSIONS;
use crate::skills::{create_source_file, write_source_file};

const GOOSE_RECIPE_PATH_ENV_VAR: &str = "GOOSE_RECIPE_PATH";

pub fn get_recipe_library_dir(is_global: bool) -> PathBuf {
    if is_global {
        Paths::config_dir().join("recipes")
    } else {
        env::current_dir().unwrap().join(".goose/recipes")
    }
}

fn local_recipe_dirs() -> Vec<PathBuf> {
    let mut local_dirs = vec![PathBuf::from(".")];

    if let Ok(recipe_path_env) = env::var(GOOSE_RECIPE_PATH_ENV_VAR) {
        let path_separator = if cfg!(windows) { ';' } else { ':' };
        local_dirs.extend(recipe_path_env.split(path_separator).map(PathBuf::from));
    }
    local_dirs.push(get_recipe_library_dir(true));
    local_dirs.push(get_recipe_library_dir(false));

    // Also scan .agents/recipes/ for consistency with the .agents/ convention
    if let Ok(cwd) = env::current_dir() {
        local_dirs.push(cwd.join(".agents/recipes"));
    }
    if let Some(home) = dirs::home_dir() {
        local_dirs.push(home.join(".agents/recipes"));
    }

    let mut dirs: Vec<PathBuf> = local_dirs
        .into_iter()
        .map(|dir| dir.canonicalize().unwrap_or(dir))
        .collect();
    dirs.sort();
    dirs.dedup();
    dirs
}

pub fn load_local_recipe_file(recipe_name: &str) -> Result<RecipeFile> {
    if RECIPE_FILE_EXTENSIONS
        .iter()
        .any(|ext| recipe_name.ends_with(&format!(".{}", ext)))
    {
        let path = PathBuf::from(recipe_name);
        return read_recipe_file(path);
    }

    if is_file_path(recipe_name) || is_file_name(recipe_name) {
        return Err(anyhow!(
            "Recipe file {} is not a json or yaml file",
            recipe_name
        ));
    }

    let search_dirs = local_recipe_dirs();
    for dir in &search_dirs {
        if let Ok(result) = load_recipe_file_from_dir(dir, recipe_name) {
            return Ok(result);
        }
    }

    let search_dirs_str = search_dirs
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(":");
    Err(anyhow!(
        "ℹ️  Failed to retrieve {}.yaml or {}.json in {}",
        recipe_name,
        recipe_name,
        search_dirs_str
    ))
}

pub fn list_local_recipes() -> Result<Vec<(PathBuf, Recipe)>> {
    let mut recipes = Vec::new();
    for dir in local_recipe_dirs() {
        if let Ok(dir_recipes) = scan_directory_for_recipes(&dir) {
            recipes.extend(dir_recipes);
        }
    }

    Ok(recipes)
}

fn is_file_path(recipe_name: &str) -> bool {
    recipe_name.contains('/')
        || recipe_name.contains('\\')
        || recipe_name.starts_with('~')
        || recipe_name.starts_with('.')
}

fn is_file_name(recipe_name: &str) -> bool {
    Path::new(recipe_name).extension().is_some()
}

fn load_recipe_file_from_dir(dir: &Path, recipe_name: &str) -> Result<RecipeFile> {
    for ext in RECIPE_FILE_EXTENSIONS {
        let recipe_path = dir.join(format!("{}.{}", recipe_name, ext));
        if let Ok(result) = read_recipe_file(recipe_path) {
            return Ok(result);
        }
    }
    Err(anyhow!(format!(
        "No {}.yaml or {}.json recipe file found in directory: {}",
        recipe_name,
        recipe_name,
        dir.display()
    )))
}

fn scan_directory_for_recipes(dir: &Path) -> Result<Vec<(PathBuf, Recipe)>> {
    let mut recipes = Vec::new();

    if !dir.exists() || !dir.is_dir() {
        return Ok(recipes);
    }

    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_file() {
            if let Some(extension) = path.extension() {
                if RECIPE_FILE_EXTENSIONS.contains(&extension.to_string_lossy().as_ref()) {
                    match Recipe::from_file_path(&path) {
                        Ok(recipe) => recipes.push((path.clone(), recipe)),
                        Err(e) => {
                            let error_message = format!(
                                "Failed to load recipe from file {}: {}",
                                path.display(),
                                e
                            );
                            tracing::error!("{}", error_message);
                        }
                    }
                }
            }
        }
    }

    Ok(recipes)
}

fn generate_recipe_filename(title: &str, recipe_library_dir: &Path) -> PathBuf {
    let base_name = title
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace() || *c == '-')
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<&str>>()
        .join("-");

    let filename = if base_name.is_empty() {
        "untitled-recipe".to_string()
    } else {
        base_name
    };

    let mut candidate = recipe_library_dir.join(format!("{}.yaml", filename));
    if fs::symlink_metadata(&candidate).is_err() {
        return candidate;
    }

    let mut counter = 1;
    loop {
        candidate = recipe_library_dir.join(format!("{}-{}.yaml", filename, counter));
        if fs::symlink_metadata(&candidate).is_err() {
            return candidate;
        }
        counter += 1;
    }
}

fn save_new_recipe_to_dir(
    title: &str,
    recipe_library_dir: &Path,
    yaml_content: &[u8],
) -> anyhow::Result<PathBuf> {
    fs::create_dir_all(recipe_library_dir)?;
    let recipe_library_dir = recipe_library_dir.canonicalize()?;
    loop {
        let file_path = generate_recipe_filename(title, &recipe_library_dir);
        let file_name = file_path
            .file_name()
            .ok_or_else(|| anyhow!("Recipe path has no file name: {}", file_path.display()))?;
        match create_source_file(&recipe_library_dir, Path::new(file_name), yaml_content) {
            Ok(()) => return Ok(file_path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
}

pub fn save_recipe_to_file(recipe: Recipe, file_path: Option<PathBuf>) -> anyhow::Result<PathBuf> {
    let recipe_library_dir = get_recipe_library_dir(true);
    let yaml_content = recipe.to_yaml()?;

    if let Some(file_path) = file_path {
        let parent = file_path
            .parent()
            .ok_or_else(|| anyhow!("Recipe path has no parent: {}", file_path.display()))?;
        let file_name = file_path
            .file_name()
            .ok_or_else(|| anyhow!("Recipe path has no file name: {}", file_path.display()))?;
        write_source_file(parent, Path::new(file_name), yaml_content.as_bytes())?;
        return Ok(file_path);
    }

    save_new_recipe_to_dir(&recipe.title, &recipe_library_dir, yaml_content.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recipe(title: &str, instructions: &str) -> Recipe {
        Recipe::builder()
            .title(title)
            .description("Test recipe")
            .instructions(instructions)
            .build()
            .unwrap()
    }

    fn write_recipe(path: &Path, title: &str, instructions: &str) {
        fs::write(path, recipe(title, instructions).to_yaml().unwrap()).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn listed_recipe_replaced_by_symlink_is_not_saved() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let recipe_root = root.path().canonicalize().unwrap();
        let recipe_path = recipe_root.join("listed.yaml");
        let outside_path = outside.path().join("outside.yaml");
        write_recipe(&recipe_path, "Listed", "original");
        write_recipe(&outside_path, "Outside", "must stay unchanged");
        let outside_content = fs::read_to_string(&outside_path).unwrap();

        let listed_path = scan_directory_for_recipes(&recipe_root).unwrap()[0]
            .0
            .clone();
        fs::remove_file(&recipe_path).unwrap();
        std::os::unix::fs::symlink(&outside_path, &recipe_path).unwrap();

        let result = save_recipe_to_file(recipe("Listed", "updated"), Some(listed_path));

        assert!(result.is_err());
        assert_eq!(fs::read_to_string(outside_path).unwrap(), outside_content);
    }

    #[test]
    fn listed_recipe_replaced_by_hard_link_is_not_saved() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let recipe_root = root.path().canonicalize().unwrap();
        let recipe_path = recipe_root.join("listed.yaml");
        let outside_path = outside.path().join("outside.yaml");
        write_recipe(&recipe_path, "Listed", "original");
        write_recipe(&outside_path, "Outside", "must stay unchanged");
        let outside_content = fs::read_to_string(&outside_path).unwrap();

        let listed_path = scan_directory_for_recipes(&recipe_root).unwrap()[0]
            .0
            .clone();
        fs::remove_file(&recipe_path).unwrap();
        fs::hard_link(&outside_path, &recipe_path).unwrap();

        let result = save_recipe_to_file(recipe("Listed", "updated"), Some(listed_path));

        assert!(result.is_err());
        assert_eq!(fs::read_to_string(outside_path).unwrap(), outside_content);
    }

    #[cfg(unix)]
    #[test]
    fn listed_recipe_with_replaced_ancestor_is_not_saved() {
        let parent = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let parent = parent.path().canonicalize().unwrap();
        let recipes = parent.join("recipes");
        let moved_recipes = parent.join("moved-recipes");
        fs::create_dir(&recipes).unwrap();
        let recipe_path = recipes.join("listed.yaml");
        let outside_path = outside.path().join("listed.yaml");
        write_recipe(&recipe_path, "Listed", "original");
        write_recipe(&outside_path, "Outside", "must stay unchanged");
        let outside_content = fs::read_to_string(&outside_path).unwrap();

        let listed_path = scan_directory_for_recipes(&recipes).unwrap()[0].0.clone();
        fs::rename(&recipes, moved_recipes).unwrap();
        std::os::unix::fs::symlink(outside.path(), &recipes).unwrap();

        let result = save_recipe_to_file(recipe("Listed", "updated"), Some(listed_path));

        assert!(result.is_err());
        assert_eq!(fs::read_to_string(outside_path).unwrap(), outside_content);
    }

    #[cfg(unix)]
    #[test]
    fn new_recipe_creation_does_not_follow_dangling_symlink_collision() {
        let recipes = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let recipe_root = recipes.path().canonicalize().unwrap();
        let outside_path = outside.path().join("missing.yaml");
        std::os::unix::fs::symlink(&outside_path, recipe_root.join("new-recipe.yaml")).unwrap();

        let saved = save_new_recipe_to_dir("New Recipe", &recipe_root, b"safe recipe").unwrap();

        assert_eq!(saved, recipe_root.join("new-recipe-1.yaml"));
        assert_eq!(fs::read_to_string(saved).unwrap(), "safe recipe");
        assert!(!outside_path.exists());
    }

    #[test]
    fn listed_regular_recipe_can_be_updated() {
        let recipes = tempfile::tempdir().unwrap();
        let recipe_root = recipes.path().canonicalize().unwrap();
        let recipe_path = recipe_root.join("listed.yaml");
        write_recipe(&recipe_path, "Listed", "original");
        let listed_path = scan_directory_for_recipes(&recipe_root).unwrap()[0]
            .0
            .clone();

        save_recipe_to_file(recipe("Listed", "updated"), Some(listed_path)).unwrap();

        assert!(fs::read_to_string(recipe_path).unwrap().contains("updated"));
    }
}
