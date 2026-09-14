use crate::recipe::build_recipe::{
    build_recipe_from_template, resolve_sub_recipe_path, RecipeError,
};
use crate::recipe::read_recipe_file_content::RecipeFile;
use crate::recipe::{RecipeParameterInputType, RecipeParameterRequirement};
use std::path::PathBuf;
use tempfile::TempDir;

#[allow(clippy::type_complexity)]
const NO_USER_PROMPT: Option<fn(&str, &str) -> Result<String, anyhow::Error>> = None;

fn setup_recipe_file(instructions_and_parameters: &str) -> (TempDir, String, PathBuf) {
    let recipe_content = format!(
        r#"{{
            "version": "1.0.0",
            "title": "Test Recipe",
            "description": "A test recipe",
            {}
        }}"#,
        instructions_and_parameters
    );
    let temp_dir = tempfile::tempdir().unwrap();
    let recipe_path = temp_dir.path().join("test_recipe.json");

    std::fs::write(&recipe_path, recipe_content).unwrap();
    let recipe_dir = temp_dir.path().to_path_buf();
    let recipe_content = std::fs::read_to_string(&recipe_path).unwrap();

    (temp_dir, recipe_content, recipe_dir)
}

fn setup_test_file(temp_dir: &TempDir, filename: &str, content: &str) -> std::path::PathBuf {
    let file_path = temp_dir.path().join(filename);
    std::fs::write(&file_path, content).unwrap();
    file_path
}

fn setup_yaml_recipe_file(instructions_and_parameters: &str) -> (TempDir, RecipeFile) {
    let recipe_content = format!(
        r#"version: "1.0.0"
title: "Test Recipe"
description: "A test recipe"
{}"#,
        instructions_and_parameters
    );
    let temp_dir = tempfile::tempdir().unwrap();
    let recipe_path = temp_dir.path().join("test_recipe.yaml");

    std::fs::write(&recipe_path, recipe_content).unwrap();

    let recipe_file = RecipeFile {
        content: std::fs::read_to_string(&recipe_path).unwrap(),
        parent_dir: temp_dir.path().to_path_buf(),
        file_path: recipe_path,
    };

    (temp_dir, recipe_file)
}

fn setup_yaml_recipe_files(
    parent_content: &str,
    child_content: &str,
) -> (TempDir, RecipeFile, RecipeFile) {
    let temp_dir = tempfile::tempdir().unwrap();
    let temp_path = temp_dir.path();

    let parent_path = temp_path.join("parent.yaml");
    std::fs::write(&parent_path, parent_content).unwrap();

    let child_path = temp_path.join("child.yaml");
    std::fs::write(&child_path, child_content).unwrap();

    let parent_recipe_file = RecipeFile {
        content: std::fs::read_to_string(&parent_path).unwrap(),
        parent_dir: temp_path.to_path_buf(),
        file_path: parent_path,
    };

    let child_recipe_file = RecipeFile {
        content: std::fs::read_to_string(&child_path).unwrap(),
        parent_dir: temp_path.to_path_buf(),
        file_path: child_path,
    };

    (temp_dir, parent_recipe_file, child_recipe_file)
}

#[test]
fn test_build_recipe_from_template_success() {
    let instructions_and_parameters = r#"
                "instructions": "Test instructions with {{ my_name }}",
                "parameters": [
                    {
                        "key": "my_name",
                        "input_type": "string",
                        "requirement": "required",
                        "description": "A test parameter"
                    }
                ]"#;

    let (_temp_dir, recipe_content, recipe_dir) = setup_recipe_file(instructions_and_parameters);

    let params = vec![("my_name".to_string(), "value".to_string())];
    let recipe =
        build_recipe_from_template(recipe_content, &recipe_dir, params, NO_USER_PROMPT).unwrap();

    assert_eq!(recipe.title, "Test Recipe");
    assert_eq!(recipe.description, "A test recipe");
    assert_eq!(recipe.instructions.unwrap(), "Test instructions with value");
    assert_eq!(recipe.parameters.as_ref().unwrap().len(), 1);
    let param = &recipe.parameters.as_ref().unwrap()[0];
    assert_eq!(param.key, "my_name");
    assert!(matches!(param.input_type, RecipeParameterInputType::String));
    assert!(matches!(
        param.requirement,
        RecipeParameterRequirement::Required
    ));
    assert_eq!(param.description, "A test parameter");
}

#[test]
fn test_build_recipe_from_template_success_variable_in_prompt() {
    let instructions_and_parameters = r#"
                "instructions": "Test instructions",
                "prompt": "My prompt {{ my_name }}",
                "parameters": [
                    {
                        "key": "my_name",
                        "input_type": "string",
                        "requirement": "required",
                        "description": "A test parameter"
                    }
                ]"#;

    let (_temp_dir, recipe_content, recipe_dir) = setup_recipe_file(instructions_and_parameters);

    let params = vec![("my_name".to_string(), "value".to_string())];
    let recipe =
        build_recipe_from_template(recipe_content, &recipe_dir, params, NO_USER_PROMPT).unwrap();

    assert_eq!(recipe.title, "Test Recipe");
    assert_eq!(recipe.description, "A test recipe");
    assert_eq!(recipe.instructions.unwrap(), "Test instructions");
    assert_eq!(recipe.prompt.unwrap(), "My prompt value");
    let param = &recipe.parameters.as_ref().unwrap()[0];
    assert_eq!(param.key, "my_name");
    assert!(matches!(param.input_type, RecipeParameterInputType::String));
    assert!(matches!(
        param.requirement,
        RecipeParameterRequirement::Required
    ));
    assert_eq!(param.description, "A test parameter");
}

#[test]
fn test_build_recipe_from_template_wrong_parameters_in_recipe_file() {
    let instructions_and_parameters = r#"
                "instructions": "Test instructions with {{ expected_param1 }} {{ expected_param2 }}",
                "parameters": [
                    {
                        "key": "wrong_param_key",
                        "input_type": "string",
                        "requirement": "required",
                        "description": "A test parameter"
                    }
                ]"#;
    let (_temp_dir, recipe_content, recipe_dir) = setup_recipe_file(instructions_and_parameters);

    let build_recipe_result =
        build_recipe_from_template(recipe_content, &recipe_dir, Vec::new(), NO_USER_PROMPT);
    assert!(build_recipe_result.is_err());
    let err = build_recipe_result.unwrap_err();
    println!("{}", err);

    match err {
        RecipeError::Invalid { source } => {
            let err_str = source.to_string();
            assert!(err_str.contains("Unnecessary parameter definitions: wrong_param_key."));
            assert!(err_str.contains("Missing definitions for parameters in the recipe file:"));
            assert!(err_str.contains("expected_param1"));
            assert!(err_str.contains("expected_param2"));
        }
        _ => panic!("Expected Invalid error"),
    }
}

#[test]
fn test_build_recipe_from_template_with_default_values_in_recipe_file() {
    let instructions_and_parameters = r#"
                "instructions": "Test instructions with {{ param_with_default }} {{ param_without_default }}",
                "parameters": [
                    {
                        "key": "param_with_default",
                        "input_type": "string",
                        "requirement": "optional",
                        "default": "my_default_value",
                        "description": "A test parameter"
                    },
                    {
                        "key": "param_without_default",
                        "input_type": "string",
                        "requirement": "required",
                        "description": "A test parameter"
                    }
                ]"#;
    let (_temp_dir, recipe_content, recipe_dir) = setup_recipe_file(instructions_and_parameters);
    let params = vec![("param_without_default".to_string(), "value1".to_string())];

    let recipe =
        build_recipe_from_template(recipe_content, &recipe_dir, params, NO_USER_PROMPT).unwrap();

    assert_eq!(recipe.title, "Test Recipe");
    assert_eq!(recipe.description, "A test recipe");
    assert_eq!(
        recipe.instructions.unwrap(),
        "Test instructions with my_default_value value1"
    );
}

#[test]
fn test_build_recipe_from_template_optional_parameters_with_empty_default_values_in_recipe_file() {
    let instructions_and_parameters = r#"
                "instructions": "Test instructions with {{ optional_param }}",
                "parameters": [
                    {
                        "key": "optional_param",
                        "input_type": "string",
                        "requirement": "optional",
                        "description": "A test parameter",
                        "default": ""
                    }
                ]"#;
    let (_temp_dir, recipe_content, recipe_dir) = setup_recipe_file(instructions_and_parameters);

    let recipe =
        build_recipe_from_template(recipe_content, &recipe_dir, Vec::new(), NO_USER_PROMPT)
            .unwrap();
    assert_eq!(recipe.title, "Test Recipe");
    assert_eq!(recipe.description, "A test recipe");
    assert_eq!(recipe.instructions.unwrap(), "Test instructions with ");
}

#[test]
fn test_build_recipe_from_template_optional_parameters_without_default_values_in_recipe_file() {
    let instructions_and_parameters = r#"
                "instructions": "Test instructions with {{ optional_param }}",
                "parameters": [
                    {
                        "key": "optional_param",
                        "input_type": "string",
                        "requirement": "optional",
                        "description": "A test parameter"
                    }
                ]"#;
    let (_temp_dir, recipe_content, recipe_dir) = setup_recipe_file(instructions_and_parameters);

    let build_recipe_result =
        build_recipe_from_template(recipe_content, &recipe_dir, Vec::new(), NO_USER_PROMPT);
    assert!(build_recipe_result.is_err());
    let err = build_recipe_result.unwrap_err();
    println!("{}", err);
    match err {
        RecipeError::Invalid { source } => {
            assert!(source.to_string().to_lowercase().contains("missing"));
        }
        _ => panic!("Expected Invalid error"),
    }
}

#[test]
fn test_build_recipe_from_template_wrong_input_type_in_recipe_file() {
    let instructions_and_parameters = r#"
                "instructions": "Test instructions with {{ param }}",
                "parameters": [
                    {
                        "key": "param",
                        "input_type": "some_invalid_type",
                        "requirement": "required",
                        "description": "A test parameter"
                    }
                ]"#;
    let params = vec![("param".to_string(), "value".to_string())];
    let (_temp_dir, recipe_content, recipe_dir) = setup_recipe_file(instructions_and_parameters);

    let build_recipe_result =
        build_recipe_from_template(recipe_content, &recipe_dir, params, NO_USER_PROMPT);
    assert!(build_recipe_result.is_err());
    let err = build_recipe_result.unwrap_err();
    match err {
        RecipeError::Invalid { source } => {
            let err_msg = source.to_string();
            eprint!("Error: {}", err_msg);
            assert!(err_msg.contains("unknown variant `some_invalid_type`"));
        }
        _ => panic!("Expected Invalid error, got: {:?}", err),
    }
}

#[test]
fn test_build_recipe_from_template_success_without_parameters() {
    let instructions_and_parameters = r#"
                "instructions": "Test instructions"
                "#;
    let (_temp_dir, recipe_content, recipe_dir) = setup_recipe_file(instructions_and_parameters);

    let recipe =
        build_recipe_from_template(recipe_content, &recipe_dir, Vec::new(), NO_USER_PROMPT)
            .unwrap();
    assert_eq!(recipe.instructions.unwrap(), "Test instructions");
    assert!(recipe.parameters.is_none());
}

#[test]
fn test_build_recipe_from_template_missing_prompt_and_instructions() {
    let instructions_and_parameters = "";
    let (_temp_dir, recipe_content, recipe_dir) = setup_recipe_file(instructions_and_parameters);

    let build_recipe_result =
        build_recipe_from_template(recipe_content, &recipe_dir, Vec::new(), NO_USER_PROMPT);
    assert!(build_recipe_result.is_err());
    let err = build_recipe_result.unwrap_err();
    println!("{}", err);

    match err {
        RecipeError::Invalid { source } => {
            let err_str = source.to_string();
            assert!(
                err_str.contains("Recipe must specify at least one of `instructions` or `prompt`.")
            );
        }
        _ => panic!("Expected Invalid error"),
    }
}

#[test]
fn test_build_recipe_from_template_rejects_empty_rendered_instructions() {
    let instructions_and_parameters = r#"
                "instructions": "{{ instructions }}",
                "parameters": [
                    {
                        "key": "instructions",
                        "input_type": "string",
                        "requirement": "optional",
                        "default": "",
                        "description": "Rendered instructions"
                    }
                ]"#;
    let (_temp_dir, recipe_content, recipe_dir) = setup_recipe_file(instructions_and_parameters);

    let error = build_recipe_from_template(recipe_content, &recipe_dir, Vec::new(), NO_USER_PROMPT)
        .unwrap_err();

    match error {
        RecipeError::Invalid { source } => assert_eq!(
            source.to_string(),
            "Recipe must specify at least one of `instructions` or `prompt`."
        ),
        other => panic!("Expected Invalid error, got: {other:?}"),
    }
}

#[test]
fn test_template_inheritance() {
    let parent_content = r#"
                version: 1.0.0
                title: Parent
                description: Parent recipe
                prompt: |
                    show me the news for day: {{ date }}
                    {% block prompt -%}
                    What is the capital of France?
                    {%- endblock %}
                    {% if is_enabled %}
                        Feature is enabled.
                    {% else %}
                        Feature is disabled.
                    {% endif %}
                parameters:
                    - key: date
                      input_type: string
                      requirement: required
                      description: date specified by the user
                    - key: is_enabled
                      input_type: boolean
                      requirement: required
                      description: whether the feature is enabled
            "#;

    let child_content = r#"
                {% extends "parent.yaml" -%}
                {% block prompt -%}
                What is the capital of Germany?
                {%- endblock %}
            "#;

    let (_temp_dir, parent_recipe_file, child_recipe_file) =
        setup_yaml_recipe_files(parent_content, child_content);

    let params = vec![
        ("date".to_string(), "today".to_string()),
        ("is_enabled".to_string(), "true".to_string()),
    ];

    let parent_recipe = build_recipe_from_template(
        parent_recipe_file.content,
        &parent_recipe_file.parent_dir,
        params.clone(),
        NO_USER_PROMPT,
    )
    .unwrap();
    assert_eq!(parent_recipe.description, "Parent recipe");
    assert_eq!(
            parent_recipe.prompt.unwrap(),
            "show me the news for day: today\nWhat is the capital of France?\n\n    Feature is enabled.\n"
        );
    assert_eq!(parent_recipe.parameters.as_ref().unwrap().len(), 2);
    assert_eq!(parent_recipe.parameters.as_ref().unwrap()[0].key, "date");
    assert_eq!(
        parent_recipe.parameters.as_ref().unwrap()[1].key,
        "is_enabled"
    );

    let child_recipe = build_recipe_from_template(
        child_recipe_file.content,
        &child_recipe_file.parent_dir,
        params,
        NO_USER_PROMPT,
    )
    .unwrap();
    assert_eq!(child_recipe.title, "Parent");
    assert_eq!(child_recipe.description, "Parent recipe");
    assert_eq!(
            child_recipe.prompt.unwrap().trim(),
            "show me the news for day: today\nWhat is the capital of Germany?\n\n    Feature is enabled."
        );
    assert_eq!(child_recipe.parameters.as_ref().unwrap().len(), 2);
    assert_eq!(child_recipe.parameters.as_ref().unwrap()[0].key, "date");
    assert_eq!(
        child_recipe.parameters.as_ref().unwrap()[1].key,
        "is_enabled"
    );
}

#[cfg(unix)]
#[test]
fn inherited_recipe_is_built_from_the_validated_snapshot() {
    use std::ffi::CString;
    use std::io::Write;
    use std::os::unix::ffi::OsStrExt;

    let temp_dir = tempfile::tempdir().unwrap();
    let parent_path = temp_dir.path().join("parent.yaml");
    let retired_fifo_path = temp_dir.path().join("parent.fifo");
    let secret_path = temp_dir.path().join("secret.txt");
    let secret = "sensitive file contents";
    std::fs::write(&secret_path, secret).unwrap();

    let fifo_path = CString::new(parent_path.as_os_str().as_bytes()).unwrap();
    // SAFETY: fifo_path is a valid, NUL-terminated path and mode contains only permission bits.
    assert_eq!(unsafe { libc::mkfifo(fifo_path.as_ptr(), 0o600) }, 0);

    let safe_parent = format!(
        r#"version: 1.0.0
title: Safe parent
description: Safe parent
instructions: "Use {{{{ TARGET }}}}"
parameters:
  - key: TARGET
    input_type: string
    requirement: optional
    description: Target value
    default: "{}"
"#,
        secret_path.display()
    );
    let swapped_parent = format!(
        r#"version: 1.0.0
title: Swapped parent
description: Swapped parent
instructions: "Use {{{{ TARGET }}}}"
retry:
  max_retries: 0
parameters:
  - key: TARGET
    input_type: file
    requirement: optional
    description: Target file
    default: "{}"
"#,
        secret_path.display()
    );

    let writer_parent_path = parent_path.clone();
    let writer = std::thread::spawn(move || {
        let mut fifo = std::fs::OpenOptions::new()
            .write(true)
            .open(&writer_parent_path)
            .unwrap();
        fifo.write_all(safe_parent.as_bytes()).unwrap();
        std::fs::rename(&writer_parent_path, retired_fifo_path).unwrap();
        std::fs::write(&writer_parent_path, swapped_parent).unwrap();
        drop(fifo);
    });

    let recipe = build_recipe_from_template(
        r#"{% extends "parent.yaml" %}"#.to_string(),
        temp_dir.path(),
        Vec::new(),
        NO_USER_PROMPT,
    )
    .unwrap();
    writer.join().unwrap();

    let expected_instructions = format!("Use {}", secret_path.display());
    assert_eq!(recipe.title, "Safe parent");
    assert_eq!(
        recipe.instructions.as_deref(),
        Some(&*expected_instructions)
    );
    assert!(!recipe.instructions.as_deref().unwrap().contains(secret));
    assert!(recipe.retry.is_none());
    assert!(matches!(
        recipe.parameters.unwrap()[0].input_type,
        RecipeParameterInputType::String
    ));
}

#[test]
fn rendered_parameter_schema_is_revalidated() {
    let parent_content = r#"
version: 1.0.0
title: Conditional parameter schema
description: Conditional parameter schema
instructions: "Use {{ TARGET }}"
parameters:
  - key: ENABLE_FILE
    input_type: boolean
    requirement: required
    description: Whether to reinterpret the target as a file
  - key: TARGET
    input_type: "{% if ENABLE_FILE %}file{% else %}string{% endif %}"
    requirement: optional
    description: Target value
    default: "/tmp/secret.txt"
"#;
    let child_content = r#"{% extends "parent.yaml" %}"#;
    let (_temp_dir, _parent_recipe_file, child_recipe_file) =
        setup_yaml_recipe_files(parent_content, child_content);

    let error = build_recipe_from_template(
        child_recipe_file.content,
        &child_recipe_file.parent_dir,
        vec![("ENABLE_FILE".to_string(), "true".to_string())],
        NO_USER_PROMPT,
    )
    .unwrap_err();

    match error {
        RecipeError::Invalid { source } => assert!(source
            .to_string()
            .contains("File parameters cannot have default values")),
        other => panic!("Expected Invalid error, got: {other:?}"),
    }
}

#[test]
fn dependency_not_loaded_during_validation_fails_closed() {
    let temp_dir = tempfile::tempdir().unwrap();
    std::fs::write(temp_dir.path().join("extra.txt"), "unvalidated content").unwrap();
    let recipe_content = r#"version: 1.0.0
title: Conditional include
description: Conditional include
instructions: |
  Safe instructions
  {% if LOAD_EXTRA %}{% include "extra.txt" %}{% endif %}
parameters:
  - key: LOAD_EXTRA
    input_type: boolean
    requirement: required
    description: Whether to load the extra template
"#;

    let error = build_recipe_from_template(
        recipe_content.to_string(),
        temp_dir.path(),
        vec![("LOAD_EXTRA".to_string(), "true".to_string())],
        NO_USER_PROMPT,
    )
    .unwrap_err();

    match error {
        RecipeError::Invalid { source } => {
            assert!(source.to_string().contains("extra.txt"));
        }
        other => panic!("Expected Invalid error, got: {other:?}"),
    }
}

mod sub_recipe_path_resolution {
    use super::*;

    fn create_recipe_file(
        temp_path: &std::path::Path,
        recipe_folder: &str,
        recipe_file_name: &str,
        content: &str,
    ) -> std::path::PathBuf {
        let recipes_dir = temp_path.join(recipe_folder);
        std::fs::create_dir_all(&recipes_dir).unwrap();
        let recipe_path = recipes_dir.join(recipe_file_name);
        std::fs::write(&recipe_path, content).unwrap();
        recipe_path
    }

    #[test]
    fn test_resolve_sub_recipe_path_relative() {
        let temp_dir = tempfile::tempdir().unwrap();
        let parent_dir = temp_dir.path();

        // Create the sub-recipe file
        let sub_recipe_content = r#"
version: 1.0.0
title: Child Recipe
description: A child recipe
instructions: Child instructions"#;
        create_recipe_file(parent_dir, "sub-recipes", "child.yaml", sub_recipe_content);

        let result = resolve_sub_recipe_path("./sub-recipes/child.yaml", parent_dir);
        assert!(result.is_ok());

        let expected_path = parent_dir
            .join("sub-recipes/child.yaml")
            .canonicalize()
            .unwrap();
        assert_eq!(result.unwrap(), expected_path.to_str().unwrap());
    }

    #[test]
    fn test_resolve_sub_recipe_path_absolute() {
        let temp_dir = tempfile::tempdir().unwrap();
        let parent_dir = temp_dir.path();

        let sub_recipe_content = r#"
version: 1.0.0
title: Absolute Recipe
description: A recipe with absolute path
instructions: Absolute instructions"#;
        let absolute_path =
            create_recipe_file(parent_dir, "absolute", "recipe.yaml", sub_recipe_content);
        let absolute_path_str = absolute_path.to_str().unwrap();

        let result = resolve_sub_recipe_path(absolute_path_str, parent_dir);
        assert!(result.is_ok());
        let expected = absolute_path.canonicalize().unwrap();
        assert_eq!(result.unwrap(), expected.to_str().unwrap());
    }

    #[test]
    fn test_resolve_sub_recipe_path_nonexistent() {
        let temp_dir = tempfile::tempdir().unwrap();
        let parent_dir = temp_dir.path();

        let result = resolve_sub_recipe_path("./sub-recipes/nonexistent.yaml", parent_dir);

        assert!(result.is_err());
        match result {
            Err(RecipeError::Invalid { source }) => {
                let error_msg = source.to_string();
                assert!(error_msg.contains("Sub-recipe file does not exist"));
                assert!(error_msg.contains("nonexistent.yaml"));
            }
            _ => panic!("Expected RecipeError::Invalid"),
        }
    }

    #[test]
    fn test_build_recipe_with_relative_sub_recipe_path() {
        let temp_dir = tempfile::tempdir().unwrap();
        let temp_path = temp_dir.path();
        let sub_recipe_content = r#"
version: 1.0.0
title: Child Recipe
description: A child recipe
instructions: Child instructions
            "#;
        create_recipe_file(temp_path, "sub-recipes", "child.yaml", sub_recipe_content);
        let main_recipe_content = r#"{
                "version": "1.0.0",
                "title": "Main Recipe",
                "description": "Main recipe with sub-recipe",
                "instructions": "Main instructions",
                "sub_recipes": [
                    {
                        "name": "child",
                        "path": "./sub-recipes/child.yaml"
                    }
                ]
            }"#;
        let main_recipe_path =
            create_recipe_file(temp_path, "main", "main.json", main_recipe_content);

        let recipe_file = RecipeFile {
            content: main_recipe_content.to_string(),
            parent_dir: temp_path.to_path_buf(),
            file_path: main_recipe_path,
        };

        let recipe = build_recipe_from_template(
            recipe_file.content,
            &recipe_file.parent_dir,
            Vec::new(),
            NO_USER_PROMPT,
        )
        .unwrap();

        assert_eq!(recipe.title, "Main Recipe");
        assert!(recipe.sub_recipes.is_some());

        let sub_recipes = recipe.sub_recipes.unwrap();
        assert_eq!(sub_recipes.len(), 1);
        assert_eq!(sub_recipes[0].name, "child");

        let expected_absolute_path = temp_path
            .join("sub-recipes/child.yaml")
            .canonicalize()
            .unwrap();
        assert_eq!(
            sub_recipes[0].path,
            expected_absolute_path.to_str().unwrap()
        );
    }
}

mod file_parameter_tests {
    use super::*;

    #[test]
    fn test_build_recipe_file_parameter_valid_paths() {
        let instructions_and_parameters = r#"instructions: "Test file content: {{ FILE_PARAM }}"
parameters:
  - key: FILE_PARAM
    input_type: file
    requirement: required
    description: A file parameter"#;

        let (temp_dir, recipe_file) = setup_yaml_recipe_file(instructions_and_parameters);

        let test_content = "Hello from file!\nThis is line 2\n    Indented line 3";
        let test_file_path = setup_test_file(&temp_dir, "test_file.txt", test_content);

        let params = vec![(
            "FILE_PARAM".to_string(),
            test_file_path.to_string_lossy().to_string(),
        )];
        let result = build_recipe_from_template(
            recipe_file.content,
            &recipe_file.parent_dir,
            params,
            NO_USER_PROMPT,
        );

        assert!(result.is_ok());
        let recipe = result.unwrap();

        let instructions = recipe.instructions.as_ref().unwrap();
        assert!(instructions.contains("Hello from file!"));
        assert!(instructions.contains("Test file content:"));
    }

    #[test]
    fn test_build_recipe_user_prompt_file_parameter_reads_file_content() {
        let instructions_and_parameters = r#"instructions: "Test file content: {{ FILE_PARAM }}"
parameters:
  - key: FILE_PARAM
    input_type: file
    requirement: user_prompt
    description: A file parameter"#;

        let (temp_dir, recipe_file) = setup_yaml_recipe_file(instructions_and_parameters);

        let test_content = "Hello from prompted file!\nThis is line 2";
        let test_file_path = setup_test_file(&temp_dir, "prompted_file.txt", test_content);
        let user_prompt = |key: &str, description: &str| -> Result<String, anyhow::Error> {
            assert_eq!(key, "FILE_PARAM");
            assert_eq!(description, "A file parameter");
            Ok(test_file_path.to_string_lossy().to_string())
        };

        let result = build_recipe_from_template(
            recipe_file.content,
            &recipe_file.parent_dir,
            Vec::new(),
            Some(user_prompt),
        );

        assert!(result.is_ok());
        let recipe = result.unwrap();

        let instructions = recipe.instructions.as_ref().unwrap();
        assert!(instructions.contains("Hello from prompted file!"));
        assert!(!instructions.contains("prompted_file.txt"));
    }

    #[test]
    fn test_build_recipe_file_parameter_nonexistent_file() {
        let instructions_and_parameters = r#"instructions: "Test file content: {{ FILE_PARAM }}"
parameters:
  - key: FILE_PARAM
    input_type: file
    requirement: required
    description: A file parameter"#;

        let (_temp_dir, recipe_file) = setup_yaml_recipe_file(instructions_and_parameters);

        let params = vec![(
            "FILE_PARAM".to_string(),
            "/nonexistent/path/file.txt".to_string(),
        )];
        let result = build_recipe_from_template(
            recipe_file.content,
            &recipe_file.parent_dir,
            params,
            NO_USER_PROMPT,
        );

        assert!(result.is_err());
        if let Err(RecipeError::Invalid { source }) = result {
            assert!(source.to_string().contains("Failed to read parameter file"));
        } else {
            panic!("Expected Invalid error");
        }
    }

    #[test]
    fn test_build_recipe_file_parameter_with_default_rejected() {
        let instructions_and_parameters = r#"instructions: "Test file content: {{ FILE_PARAM }}"
parameters:
  - key: FILE_PARAM
    input_type: file
    requirement: required
    description: A file parameter
    default: "/etc/passwd""#;

        let (_temp_dir, recipe_file) = setup_yaml_recipe_file(instructions_and_parameters);

        let params = vec![];
        let result = build_recipe_from_template(
            recipe_file.content,
            &recipe_file.parent_dir,
            params,
            NO_USER_PROMPT,
        );

        assert!(result.is_err());
        if let Err(RecipeError::Invalid { source }) = result {
            assert!(source
                .to_string()
                .contains("File parameters cannot have default values"));
        } else {
            panic!("Expected Invalid error for file parameter with default");
        }
    }
}

#[test]
fn test_build_recipe_from_template_invalid_retry_config() {
    let instructions_and_parameters = r#"
                "instructions": "Test instructions",
                "retry": {
                    "max_retries": 0,
                    "checks": []
                }"#;
    let (_temp_dir, recipe_content, recipe_dir) = setup_recipe_file(instructions_and_parameters);

    let build_recipe_result =
        build_recipe_from_template(recipe_content, &recipe_dir, Vec::new(), NO_USER_PROMPT);
    assert!(build_recipe_result.is_err());
    let err = build_recipe_result.unwrap_err();

    match err {
        RecipeError::Invalid { source } => {
            assert_eq!(
                source.to_string(),
                "Invalid retry configuration: max_retries must be greater than 0"
            );
        }
        _ => panic!("Expected Invalid error, got: {:?}", err),
    }
}
