use std::path::{Path, PathBuf};

use goose::recipe::validate_recipe::{
    validate_recipe_for_scheduling, validate_recipe_template_from_content, RecipeFileFormat,
};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("goose crate must be inside the workspace crates directory")
        .to_path_buf()
}

fn collect_yaml_files(directory: &Path, paths: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", directory.display()))
    {
        let path = entry.expect("failed to read directory entry").path();
        if path.is_dir() {
            collect_yaml_files(&path, paths);
        } else if path
            .extension()
            .is_some_and(|extension| extension == "yaml")
        {
            paths.push(path);
        }
    }
}

#[test]
fn checked_in_recipes_match_cli_validation_for_scheduling() {
    let root = workspace_root();
    let recipes_root = root.join("documentation/src/pages/recipes/data/recipes");
    let mut paths = Vec::new();
    collect_yaml_files(&recipes_root, &mut paths);
    paths.sort();
    assert!(!paths.is_empty(), "expected checked-in recipe fixtures");

    for path in paths {
        let content = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let recipe_dir = path
            .parent()
            .expect("recipe path must have a parent")
            .to_string_lossy()
            .into_owned();

        let cli_result = validate_recipe_template_from_content(&content, Some(recipe_dir.clone()));
        let scheduling_result =
            validate_recipe_for_scheduling(&content, Some(recipe_dir), RecipeFileFormat::Yaml);

        assert_eq!(
            scheduling_result.is_ok(),
            cli_result.is_ok(),
            "validation verdict differed for {}: CLI={:?}, scheduling={:?}",
            path.display(),
            cli_result.err(),
            scheduling_result.err()
        );
    }
}
