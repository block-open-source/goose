---
sidebar_position: 2
title: Recipe Reference Guide
sidebar_label: Recipe Reference
description: Complete technical reference for creating and customizing recipes in goose
---

import Tabs from '@theme/Tabs';
import TabItem from '@theme/TabItem';

Recipes are reusable goose configurations that package up instructions and settings so the setup can be easily shared and launched by others.

## Recipe File Format

Recipes can be defined in:
- `.yaml` (recommended) and `.yml` files
- `.json` files

:::info
`.yml` files aren't supported by goose CLI.
:::

See [Reusable Recipes](/docs/guides/recipes/session-recipes) to learn how to create, use, and manage recipes.

## Recipe Location

Recipes can be loaded from:

1. Local filesystem:
   - Current directory
   - Directories specified in [`GOOSE_RECIPE_PATH`](/docs/guides/environment-variables#recipe-configuration) environment variable
   
2. GitHub repositories:
   - Configure using [`GOOSE_RECIPE_GITHUB_REPO`](/docs/guides/environment-variables#recipe-configuration) configuration key
   - Requires GitHub CLI (`gh`) to be installed and authenticated

## Core Recipe Schema

Recipes follow this schema structure:

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `description` | String | ✅ | A detailed description of what the recipe does |
| `instructions` | String | ✅*  | Template instructions that can include parameter substitutions |
| `prompt` | String| ✅*   | A template prompt that can include parameter substitutions. Required in [headless](/docs/tutorials/headless-goose) (non-interactive) mode. |
| `title` | String | ✅ | A short title describing the recipe |
| [`activities`](#activities) | Array | - | List of example prompts that can include parameter substitutions. Activities appear as clickable bubbles in goose Desktop. |
| [`extensions`](#extensions) | Array | - | List of extension configurations |
| [`parameters`](#parameters) | Array | - | List of parameter definitions for dynamic recipes |
| [`response`](#response) | Object | - | Structured output schema for automation workflows |
| [`retry`](#retry) | Object | - | Configuration for automated retry logic with success validation |
| [`settings`](#settings) | Object | - | Configuration for model provider, model name, and other settings |
| [`sub_recipes`](#subrecipes) | Array | - | List of subrecipes |
| `version` | String | - | The recipe format version, defaults to "1.0.0" if omitted |

*At least one of `instructions` or `prompt` must be provided.

## Field Specifications

### Activities

The `activities` field defines an optional message and clickable activity bubbles (buttons) that appears when a recipe is opened in goose Desktop.

:::info Desktop only
Activities are a Desktop-only feature. When recipes with activities are run via the CLI or as a scheduled job, the `activities` field is ignored and has no effect on recipe execution.
:::

#### Activity Types

Activities can be defined in two ways:

1. **Message Activity**: Displays the markdown-formatted activity text in an info box above the activity bubbles. For example:
   
   ```
   activities:
     - "message: **Welcome!** Here's what I can help with:\n\n• 📊 Data analysis\n• 🔍 Code review\n• 📝 Documentation\n\nSelect an option below to begin."
   ```
   
   Only include one `message:` prefixed activity. Additional `message:` prefixed activities become regular clickable bubbles (and display the literal "message:" text).

2. **Button Activities**: Text to display in activity bubbles, which send the activity text as a prompt when clicked

#### Parameter Substitution

Activities support [parameter substitution](#parameters), allowing you to create dynamic, personalized activity bubbles. After users provide parameter values in the **Recipe Parameters** dialog, the values are substituted into the activity text before the bubbles are displayed.

#### Example Configuration

<Tabs groupId="format">
  <TabItem value="yaml" label="YAML" default>
    ```yaml
    version: "1.0.0"
    title: "Code Review Assistant"
    description: "Review code with customizable focus areas"
    parameters:
      - key: language
        input_type: string
        requirement: required
        description: "Programming language to review"
      - key: focus
        input_type: string
        requirement: optional
        default: "best practices"
        description: "Review focus area"

    activities:
      - "message: Click an option below to start reviewing {{ language }} code with a focus on {{ focus }}."
      - "Review the current file for {{ focus }}"
      - "Suggest improvements for {{ language }} code quality"
      - "Check for security vulnerabilities"
      - "Generate unit tests"
    ```
  </TabItem>
  <TabItem value="json" label="JSON">
    ```json
    {
      "version": "1.0.0",
      "title": "Code Review Assistant",
      "description": "Review code with customizable focus areas",
      "parameters": [
        {
          "key": "language",
          "input_type": "string",
          "requirement": "required",
          "description": "Programming language to review"
        },
        {
          "key": "focus",
          "input_type": "string",
          "requirement": "optional",
          "default": "best practices",
          "description": "Review focus area"
        }
      ],
      "activities": [
        "message: Click an option below to start reviewing {{ language }} code with a focus on {{ focus }}.",
        "Review the current file for {{ focus }}",
        "Suggest improvements for {{ language }} code quality",
        "Check for security vulnerabilities",
        "Generate unit tests"
      ]
    }
    ```
  </TabItem>
</Tabs>

In this example:
- The message activity displays instructions with substituted parameter values, for example: "Click an option below to start reviewing rust code with a focus on best practices."
- The first two activity bubbles use parameter substitution, for example: "Review the current file for best practices"
- The last two activity bubbles are static prompts that work regardless of parameters

### Extensions

The `extensions` field allows you to specify which Model Context Protocol (MCP) servers and other extensions the recipe needs to function. Each extension in the `extensions` array has the following schema:

#### Extension Schema

| Field | Type | Description |
|-------|------|-------------|
| `type` | String | Type of extension (e.g., "stdio") |
| `name` | String | Unique name for the extension |
| `cmd` | String | Command to run the extension |
| `args` | Array | List of arguments for the command |
| `env_keys` | Array | (Optional) Names of environment variables required by the extension |
| `timeout` | Number | Timeout in seconds |
| `bundled` | Boolean | (Optional) Whether the extension is bundled with goose |
| `description` | String | Description of what the extension does |
| `available_tools` | Array | List of tool names within the extension that will be available. When not specified all will be available |

#### Extension Types

- **`stdio`**: Standard I/O client with command and arguments
- **`builtin`**: Built-in extension that is part of the bundled goose MCP server
- **`platform`**: Platform extensions that run in the agent process
- **`streamable_http`**: Streamable HTTP client with URI endpoint

:::note Summon Extension and Subagents
The `delegate` and `load` tools are provided by the `summon` platform extension. When a recipe specifies an explicit `extensions` block, only the listed extensions are available — default platform extensions like `summon` are not automatically included. If your recipe needs subagent delegation, add `summon` to your extensions list:

```yaml
extensions:
  - type: platform
    name: summon
```

Recipes that define [`sub_recipes`](/docs/guides/recipes/subrecipes) have `summon` auto-injected and do not need to list it explicitly.
:::

#### Example Extensions Configuration

<Tabs groupId="format">
  <TabItem value="yaml" label="YAML" default>

```yaml
extensions:
  - type: stdio
    name: codesearch
    cmd: uvx
    args:
      - mcp_codesearch@latest
    timeout: 300
    bundled: true
    description: "Query https://codesearch.sqprod.co/ directly from goose"
  
  - type: stdio
    name: presidio
    timeout: 300
    cmd: uvx
    args:
      - 'mcp_presidio@latest'
    available_tools:
      - query_logs

  - type: stdio
    name: github-mcp
    cmd: github-mcp-server
    args: []
    env_keys:
      - GITHUB_PERSONAL_ACCESS_TOKEN
    timeout: 60
    description: "GitHub MCP extension for repository operations"
    
```

  </TabItem>
  <TabItem value="json" label="JSON">

```json
{
  "extensions": [
    {
      "type": "stdio",
      "name": "codesearch",
      "cmd": "uvx",
      "args": ["mcp_codesearch@latest"],
      "timeout": 300,
      "bundled": true,
      "description": "Query https://codesearch.sqprod.co/ directly from goose"
    },
    {
      "type": "stdio",
      "name": "presidio",
      "timeout": 300,
      "cmd": "uvx",
      "args": ["mcp_presidio@latest"],
      "available_tools": ["query_logs"]
    },
    {
      "type": "stdio",
      "name": "github-mcp",
      "cmd": "github-mcp-server",
      "args": [],
      "env_keys": ["GITHUB_PERSONAL_ACCESS_TOKEN"],
      "timeout": 60,
      "description": "GitHub MCP extension for repository operations"
    }
  ]
}
```

  </TabItem>
</Tabs>

#### Extension environment variables

Extensions can declare the names of required environment variables in `env_keys`. goose resolves these values when the extension starts, using an environment variable first and then goose secret storage (the system keyring, or `secrets.yaml` when the keyring is disabled).

Recipe loading does not prompt for missing values. Configure them before starting the recipe; if a required value is unavailable, the extension reports an initialization error.

:::info
`env_keys` can include secrets such as API keys as well as non-secret configuration such as API endpoints.
:::

### Parameters

The `parameters` field allows you to create dynamic, reusable recipes that can be customized for different contexts. Parameters define placeholders that users fill in when running the recipe, making the recipe more flexible and adaptable.

Parameter substitution uses Jinja-style template syntax with `{{ parameter_name }}` placeholders. Each parameter in the `parameters` array has the following schema:

#### Parameter Schema

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `key` | String | ✅ | Unique identifier for the parameter |
| `input_type` | String | ✅ | Type of input: `"string"` (default), `"number"`, `"boolean"`, `"date"`, `"file"`, or `"select"` |
| `requirement` | String | ✅ | One of: "required", "optional", or "user_prompt" |
| `description` | String | ✅ | Human-readable description of the parameter |
| `default` | String | - | Default value for optional parameters |
| `options` | Array | - | List of available choices (required for `select` input type) |

#### Parameter Requirements

- `required`: Parameter must be provided when using the recipe
- `optional`: Can be omitted if a default value is specified
- `user_prompt`: Will interactively prompt the user for input if not provided

The `required` and `optional` parameters work best for recipes opened in goose Desktop. If a value isn't provided for a `user_prompt` parameter, the parameter won't be substituted and may appear as literal `{{ parameter_name }}` text in the recipe output.

#### Input Types

- `string`: Default type. The parameter value is used as-is in template substitution
- `number`: Numeric values. Desktop UI provides number input validation
- `boolean`: True/false values. Desktop UI shows dropdown with "True"/"False" options
- `date`: Date values. Currently renders as text input
- `file`: The parameter value should be a file path. goose reads the file contents and substitutes the actual content (not the path) into the template
- `select`: Dropdown selection with predefined options. Requires `options` field

**Example:**
```yaml
parameters:
  - key: max_files
    input_type: number
    requirement: optional
    default: "10"
    description: "Maximum files to process"
  
  - key: output_format
    input_type: select
    requirement: required
    description: "Choose output format"
    options:
      - json
      - markdown
      - csv
  
  - key: enable_debug
    input_type: boolean
    requirement: optional
    default: "false"
    description: "Enable debug mode"
  
  - key: source_code
    input_type: file
    requirement: required
    description: "Path to the source code file to analyze"

prompt: "Process {{ max_files }} files in {{ output_format }} format. Debug: {{ enable_debug }}. Code:\n\n{{ source_code }}"
```

:::important
- Optional parameters MUST have a default value specified
- Required parameters cannot have default values
- File parameters cannot have default values regardless of requirement type to prevent unintended importing of sensitive files
- Select parameters MUST have an `options` field with available choices
- Parameter keys must match any template variables used in instructions, prompt, or activities
:::

#### Parameter Substitution in Desktop

When a recipe with parameters is opened in goose Desktop, users are presented with a **Recipe Parameters** dialog where they can:
- Provide values for required parameters
- Modify or accept default values for optional parameters  
- Enter values for `user_prompt` parameters

Once parameter values are submitted, they are substituted into the recipe's `instructions`, `prompt`, and `activities` fields before the recipe starts.

### Response

The `response` field enables recipes to enforce a final structured JSON output. When you specify a `json_schema`, goose will:

1. **Validate the output**: Validates the output JSON against your JSON schema with basic JSON schema validations
2. **Final structured output**: Ensure the final output of the agent is a response matching your JSON structure

This feature is designed for **non-interactive automation** to ensure consistent, parseable output. Recipes can produce structured output when run from either the goose CLI or goose Desktop. See [use cases and ideas for automation workflows](/docs/guides/recipes/session-recipes#structured-output-for-automation).

#### Response Schema

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `json_schema` | Object | ✅ | [JSON schema](https://json-schema.org/) for output validation |

#### Basic Structure

```yaml
response:
  json_schema:
    type: object
    properties:
      # Define your fields here, with their type and description
    required:
      # List required field names
```

#### Simple Example

```yaml
version: "1.0.0"
title: "Task Summary"
description: "Summarize completed tasks"
prompt: "Summarize the tasks you completed"
response:
  json_schema:
    type: object
    properties:
      summary:
        type: string
        description: "Brief summary of work done"
      tasks_completed:
        type: number
        description: "Number of tasks finished"
      next_steps:
        type: array
        items:
          type: string
        description: "Recommended next actions"
    required:
      - summary
      - tasks_completed
```

### Retry

The `retry` field enables recipes to automatically retry execution if success criteria are not met. This is useful for recipes that might need multiple attempts to achieve their goal, or for implementing automated validation and recovery workflows.

#### Retry Schema

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `max_retries` | Number | ✅ | Maximum number of retry attempts |
| `checks` | Array | ✅ | List of success check configurations |
| `timeout_seconds` | Number | - | Timeout for success check commands (default: 300 seconds) |
| `on_failure_timeout_seconds` | Number | - | Timeout for on_failure commands (default: 600 seconds) |
| `on_failure` | String | - | Shell command to run when a retry attempt fails |

#### Success Check Configuration

Each success check in the `checks` array has the following schema:

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `type` | String | ✅ | Type of check - currently only "shell" is supported |
| `command` | String | ✅ | Shell command to execute for validation (must exit with code 0 for success) |

#### How Retry Logic Works

1. **Recipe Execution**: The recipe runs normally with the provided instructions
2. **Success Validation**: After completion, all success checks are executed in order
3. **Retry Decision**: If any success check fails and retry attempts remain:
   - Execute the on_failure command (if configured)
   - Reset the agent's message history to initial state
   - Increment retry counter and restart execution
4. **Completion**: Process stops when either:
   - All success checks pass (success)
   - Maximum retry attempts are reached (failure)

#### Basic Retry Example

```yaml
version: "1.0.0"
title: "Counter Increment Task"
description: "Increment a counter until it reaches target value"
prompt: "Increment the counter value in /tmp/counter.txt by 1."

retry:
  max_retries: 5
  timeout_seconds: 10
  checks:
    - type: shell
      command: "test $(cat /tmp/counter.txt 2>/dev/null || echo 0) -ge 3"
  on_failure: "echo 'Counter is at:' $(cat /tmp/counter.txt 2>/dev/null || echo 0) '(need 3 to succeed)'"
```

#### Advanced Retry Example

```yaml
version: "1.0.0"
title: "Service Health Check"
description: "Start service and verify it's running properly"
prompt: "Start the web service and verify it responds to health checks"

retry:
  max_retries: 3
  timeout_seconds: 30
  on_failure_timeout_seconds: 60
  checks:
    - type: shell
      command: "curl -f http://localhost:8080/health"
    - type: shell  
      command: "pgrep -f 'web-service' > /dev/null"
  on_failure: "systemctl stop web-service || killall web-service"
```

#### Environment Variables

You can configure retry behavior globally using environment variables:

- `GOOSE_RECIPE_RETRY_TIMEOUT_SECONDS`: Global timeout for success check commands
- `GOOSE_RECIPE_ON_FAILURE_TIMEOUT_SECONDS`: Global timeout for on_failure commands

These environment variables are overridden by recipe-specific timeout configurations.

### Settings

The `settings` field allows you to configure the AI model and provider settings for the recipe. This overrides the default configuration when the recipe is executed.

#### Settings Schema

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `goose_provider` | String | - | The AI provider to use (e.g., "anthropic", "openai") |
| `goose_model` | String | - | The specific model name to use |
| `temperature` | Number | - | The temperature setting for the model (typically 0.0-1.0) |
| `max_turns` | Number | - | Maximum number of turns for subagent tasks created by this recipe |

#### Understanding max_turns

The `max_turns` setting controls how many iterations an agent can perform before stopping. When set in a recipe's settings, it applies to that recipe's execution and any subagents or subrecipes it creates (unless they specify their own value).

**Configuration precedence (highest to lowest):**
1. Subagent tool call override
2. Recipe `settings.max_turns`
3. `GOOSE_SUBAGENT_MAX_TURNS` environment variable
4. Default value (1000 for main recipes, 25 for subagents)

**Common use cases:** Limit execution time for automated workflows, prevent runaway subagents, control resource usage in scheduled jobs.

#### Example Settings Configuration

```yaml
settings:
  goose_provider: "anthropic"
  goose_model: "claude-sonnet-4-20250514"
  temperature: 0.7
  max_turns: 50
```

```yaml
settings:
  goose_provider: "openai"
  goose_model: "gpt-4o"
  temperature: 0.3
```

:::note
Settings specified in a recipe will override your default goose configuration when that recipe is executed. If no settings are specified, goose will use your configured defaults.
:::

### Subrecipes

The `sub_recipes` field specifies the [subrecipes](/docs/guides/recipes/subrecipes) that the main recipe calls to perform specific tasks. Each subrecipe in the `sub_recipes` array has the following schema:

#### Subrecipe Schema

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | String | ✅ | Unique identifier for the subrecipe |
| `path` | String | ✅ | Relative or absolute path to the subrecipe file |
| `values` | Object | - | Pre-configured parameter values that are passed to the subrecipe |
| `sequential_when_repeated` | Boolean | - | Forces sequential execution of multiple subrecipe instances. See [Running Subrecipes In Parallel](/docs/tutorials/subrecipes-in-parallel) for details |
| `description` | String | - | Optional description of the subrecipe |

#### Example Subrecipe Configuration

```yaml
sub_recipes:
  - name: "security_scan"
    path: "./subrecipes/security-analysis.yaml"
    values:  # in key-value format: {parameter_name}: {parameter_value}
      scan_level: "comprehensive"
      include_dependencies: "true"
  
  - name: "quality_check"
    path: "./subrecipes/quality-analysis.yaml"
    description: "Performs code quality analysis"
```

## Desktop Metadata Fields

Recipes saved from goose Desktop include additional metadata fields. These fields are used by the Desktop app for organization and management but are ignored by CLI operations. 

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `recipe` | Object | ✅ |  Contains all recipe fields (`title`, `description`, `instructions`, etc.) |
| `name` | String | ✅ |  Display name used in Recipe Library |
| `isGlobal` | Boolean | ✅ |  Whether the recipe is available globally or locally to a project |
| `lastModified` | String | ✅ |  ISO timestamp of when the recipe was last modified |
| `isArchived` | Boolean | ✅ |  Whether the recipe is archived in the Desktop interface |

<details>
<summary>CLI and Desktop Format Examples</summary>

**CLI Format**

<Tabs groupId="format">
  <TabItem value="yaml" label="YAML" default>

```yaml
version: "1.0.0"
title: "Code Review Assistant"
description: "Automated code review with best practices"
instructions: "You are a code reviewer..."
prompt: "Review the code in this repository"
extensions: []
```

  </TabItem>
  <TabItem value="json" label="JSON">

```json
{
  "version": "1.0.0",
  "title": "Code Review Assistant",
  "description": "Automated code review with best practices",
  "instructions": "You are a code reviewer...",
  "prompt": "Review the code in this repository",
  "extensions": []
}
```

  </TabItem>
</Tabs>

**Desktop Format**

<Tabs groupId="format">
  <TabItem value="yaml" label="YAML" default>

```yaml
name: "Code Review Assistant"
recipe:
  version: "1.0.0"
  title: "Code Review Assistant"
  description: "Automated code review with best practices"
  instructions: "You are a code reviewer..."
  prompt: "Review the code in this repository"
  extensions: []
isGlobal: true
lastModified: 2025-07-02T03:46:46.778Z
isArchived: false
```

  </TabItem>
  <TabItem value="json" label="JSON">

```json
{
  "name": "Code Review Assistant",
  "recipe": {
    "version": "1.0.0",
    "title": "Code Review Assistant",
    "description": "Automated code review with best practices",
    "instructions": "You are a code reviewer...",
    "prompt": "Review the code in this repository",
    "extensions": []
  },
  "isGlobal": true,
  "lastModified": "2025-07-02T03:46:46.778Z",
  "isArchived": false
}
```

  </TabItem>
</Tabs>

</details>

## Template Support

Recipes support Jinja-style template syntax in `instructions`, `prompt`, and `activities` fields for parameter substitution:

```yaml
instructions: "Follow these steps with {{ parameter_name }}"
prompt: "Your task is to {{ action }}"
activities:
  - "Process {{ parameter_name }} with {{ action }}"
```

Advanced template features include:
- [Escaping template variables](#escaping-template-variables) for literal output
- [Template inheritance](#template-inheritance) using `{% extends "parent.yaml" %}`
- Blocks that can be defined and overridden:
  ```yaml
  {% block content %}
  Default content
  {% endblock %}
  ```
- [`indent()` template filter](#indent-filter-for-multi-line-values)

### Escaping Template Variables

To include literal template syntax (like `{{ variable }}`) in your recipe without parameter substitution, wrap it in single quotes:

```yaml
prompt: |
  This will be substituted: {{ actual_parameter }}
  This will appear literally: {{'{{example_variable}}'}}
```

**Example:** Generate a configuration file template

```yaml
version: "1.0.0"
title: "Generate Config Template"
description: "Generate a template with placeholder values"
parameters:
  - key: app_name
    input_type: string
    requirement: required
    description: "Application name"

prompt: |
  Create a config.yaml file for {{ app_name }} with these placeholder variables:
  - {{'{{API_KEY}}'}} for the API key
  - {{'{{DATABASE_URL}}'}} for the database connection
  - {{'{{PORT}}'}} for the server port
```

### Template Inheritance

Use `{% extends "parent.yaml" %}` for template inheritance:

**Parent recipe (`parent.yaml`):**
```yaml
version: "1.0.0"
title: "Parent Recipe"
description: "Base recipe template"
prompt: |
  {% block prompt %}
  Default prompt text
  {% endblock %}
```

**Child recipe:**
```yaml
{% extends "parent.yaml" %}
{% block prompt %}
Modified prompt text
{% endblock %}
```

### indent() Filter For Multi-Line Values

Use the `indent()` filter to ensure multi-line parameter values are properly indented and can be resolved as valid JSON or YAML format. This example uses `{{ raw_data | indent(2) }}` to specify an indentation of two spaces when passing data to a subrecipe:

```yaml
sub_recipes:
  - name: "analyze"
    path: "./analyze.yaml"
    values:
      content: |
        {{ raw_data | indent(2) }}
```

### Built-in Parameters

Built-in template parameters are automatically supported and don't need to be defined in the `parameters` array.

| Parameter | Description |
|-----------|-------------|
| `recipe_dir` | Automatically set to the directory containing the recipe file. Use it to reference companion files, for example: `{{ recipe_dir }}/style-guide.md` |

## Validation Rules

Validation rules from [`validate_recipe.rs`](https://github.com/aaif-goose/goose/blob/main/crates/goose/src/recipe/validate_recipe.rs) are enforced when loading recipes and used by the [`goose recipe validate`](/docs/guides/goose-cli-commands#recipe) subcommand:

### Recipe-Level Validation

- `validate_prompt_or_instructions` - At least one of `instructions` or `prompt` must be present
- `validate_json_schema` - JSON response schema must be valid if `response.json_schema` is specified

### Parameter Validation

- `validate_parameters_in_template` - All template variables must have corresponding parameter definitions, and all defined parameters must be used (no unused parameters)
- `validate_optional_parameters` - Optional parameters must have default values
- `validate_optional_parameters` - File parameters cannot have default values to prevent importing sensitive files

:::info
Basic field requirements (required fields, types, character limits) are documented in the [Core Recipe Schema](#core-recipe-schema) table.
:::

## Complete Recipe Example

<Tabs groupId="format">
  <TabItem value="yaml" label="YAML" default>

```yaml
version: "1.0.0"
title: "Example Recipe"
description: "A sample recipe demonstrating the format"
instructions: "Process {{ file_count }} files using {{ required_param }} and output in {{ output_format }} format. Configuration: {{ config_file }}"
prompt: "Start processing with the provided parameters"
parameters:
  - key: required_param
    input_type: string
    requirement: required
    description: "A required text parameter"
  
  - key: file_count
    input_type: number
    requirement: optional
    default: 10
    description: "Maximum number of files to process"
  
  - key: output_format
    input_type: select
    requirement: required
    description: "Choose the output format"
    options:
      - json
      - markdown
      - csv
  
  - key: config_file
    input_type: file
    requirement: required
    description: "Path to configuration file"

extensions:
  - type: stdio
    name: codesearch
    cmd: uvx
    args:
      - mcp_codesearch@latest
    timeout: 300
    bundled: true
    description: "Query codesearch directly from goose"

settings:
  goose_provider: "anthropic"
  goose_model: "claude-sonnet-4-20250514"
  temperature: 0.7
  max_turns: 100

retry:
  max_retries: 3
  timeout_seconds: 30
  checks:
    - type: shell
      command: "echo 'Task validation check passed'"
  on_failure: "echo 'Retry attempt failed, cleaning up...'"

response:
  json_schema:
    type: object
    properties:
      result:
        type: string
        description: "The main result of the task"
      details:
        type: array
        items:
          type: string
        description: "Additional details of steps taken"
    required:
      - result
      - details
```

  </TabItem>
  <TabItem value="json" label="JSON">

```json
{
  "version": "1.0.0",
  "title": "Example Recipe",
  "description": "A sample recipe demonstrating the format",
  "instructions": "Process {{ file_count }} files using {{ required_param }} and output in {{ output_format }} format. Configuration: {{ config_file }}",
  "prompt": "Start processing with the provided parameters",
  "parameters": [
    {
      "key": "required_param",
      "input_type": "string",
      "requirement": "required",
      "description": "A required text parameter"
    },
    {
      "key": "file_count",
      "input_type": "number",
      "requirement": "optional",
      "default": "10",
      "description": "Maximum number of files to process"
    },
    {
      "key": "output_format",
      "input_type": "select",
      "requirement": "required",
      "description": "Choose the output format",
      "options": ["json", "markdown", "csv"]
    },
    {
      "key": "config_file",
      "input_type": "file",
      "requirement": "required",
      "description": "Path to configuration file"
    }
  ],
  "extensions": [
    {
      "type": "stdio",
      "name": "codesearch",
      "cmd": "uvx",
      "args": ["mcp_codesearch@latest"],
      "timeout": 300,
      "bundled": true,
      "description": "Query codesearch directly from goose"
    }
  ],
  "settings": {
    "goose_provider": "anthropic",
    "goose_model": "claude-sonnet-4-20250514",
    "temperature": 0.7,
    "max_turns": 100
  },
  "retry": {
    "max_retries": 3,
    "timeout_seconds": 30,
    "checks": [
      {
        "type": "shell",
        "command": "echo 'Task validation check passed'"
      }
    ],
    "on_failure": "echo 'Retry attempt failed, cleaning up...'"
  },
  "response": {
    "json_schema": {
      "type": "object",
      "properties": {
        "result": {
          "type": "string",
          "description": "The main result of the task"
        },
        "details": {
          "type": "array",
          "items": {
            "type": "string"
          },
          "description": "Additional details of steps taken"
        }
      },
      "required": ["result", "details"]
    }
  }
}
```

  </TabItem>
</Tabs>

## Error Handling

Common errors to watch for:

- Missing required parameters
- Optional parameters without default values
- Template variables without parameter definitions
- Invalid YAML/JSON syntax
- Missing required fields
- Invalid extension configurations
- Invalid retry configuration (missing required fields, invalid shell commands)

When these occur, goose will provide helpful error messages indicating what needs to be fixed.

### Retry-Specific Errors

- **Invalid success checks**: Shell commands that cannot be executed or have syntax errors
- **Timeout errors**: Success checks or on_failure commands that exceed their timeout limits
- **Max retries exceeded**: When all retry attempts are exhausted without success
- **Missing required retry fields**: When `max_retries` or `checks` are not specified

## Learn More
Check out the [Recipes](/docs/guides/recipes) guide for more docs, tools, and resources to help you master goose recipes.
