---
sidebar_position: 1
title: Reusable Recipes
description: "Package tools, goals, and instructions as a reusable recipe that others can launch with a single click"
---

import Tabs from '@theme/Tabs';
import TabItem from '@theme/TabItem';
import { PanelLeft, ChefHat, SquarePen, Link, Clock, Terminal, Share2 } from 'lucide-react';
import RecipeFields from '@site/src/components/RecipeFields';

Recipes package tools, goals, and instructions into reusable workflows that others (or future you) can launch with a single click.

## Create Recipe

<Tabs groupId="interface">
  <TabItem value="ui" label="goose Desktop" default>

  1. Click the <PanelLeft className="inline" size={16} /> button in the top-left to open the sidebar
  2. Click `Recipes` in the sidebar
  3. Click `Create Recipe`
  4. In the dialog that opens, fill in the recipe fields as needed:
     <RecipeFields />
  5. When you're finished, you can:
     - Copy the recipe link to share the recipe with others
     - Click `Save Recipe` to save the recipe to your Recipe Library
     - Click `Save & Run Recipe` to save and immediately run the recipe in a new session

  </TabItem>

  <TabItem value="cli" label="goose CLI">
   Create a JSON or YAML file in your editor using the recipe structure below.

   <details>
   <summary>recipe file structure</summary>

   ```yaml
   # Required fields
   version: 1.0.0
   title: $title
   description: $description
   instructions: $instructions    # Define the model's behavior

   # Optional fields
   prompt: $prompt                # Initial message to start with
   extensions:                    # Tools the recipe needs
   - $extensions
   activities:                    # Example prompts to display in the Desktop app
   - $activities
   settings:                      # Additional settings
     goose_provider: $provider    # Provider to use for this recipe
     goose_model: $model          # Specific model to use for this recipe
     temperature: $temperature    # Model temperature setting for this recipe (0.0 to 1.0)
   retry:                         # Automated retry logic with success validation
     max_retries: $max_retries    # Maximum number of retry attempts
     checks:                      # Success validation checks
     - type: shell
       command: $validation_command
     on_failure: $cleanup_command # Optional cleanup command on failure
   ```
   </details>

    For detailed descriptions and example configurations of all recipe fields, see the [Recipe Reference Guide](/docs/guides/recipes/recipe-reference).

   :::tip Validate Your Recipe
   You should [validate your recipe](#validate-recipe) to verify that it's complete and properly formatted.
   :::

   #### Optional Parameters

   You may add parameters to a recipe, which will require users to fill in data when running the recipe. Parameters can be added to any part of the recipe (instructions, prompt, activities, etc).

   To use parameters:
   1. Add template variables using `{{ variable_name }}` syntax in your recipe content
   2. Define each parameter in the `parameters` section of your YAML file

   <details>
   <summary>Example recipe with parameters</summary>

   ```yaml
   version: 1.0.0
   title: "{{ project_name }} Code Review" # Wrap the value in quotes if it starts with template syntax to avoid YAML parsing errors
   description: Automated code review for {{ project_name }} with {{ language }} focus
   instructions: You are a code reviewer specialized in {{ language }} development.
   prompt: |
      Apply the following standards:
      - Complexity threshold: {{ complexity_threshold }}
      - Required test coverage: {{ test_coverage }}%
      - Style guide: {{ style_guide }}
   activities:
   - "Review {{ language }} code for complexity"
   - "Check test coverage against {{ test_coverage }}% requirement"
   - "Verify {{ style_guide }} compliance"
   settings:                     
     goose_provider: "anthropic"   
     goose_model: "claude-3-7-sonnet-latest"          
     temperature: 0.7 
   parameters:
   - key: project_name
     input_type: string
     requirement: required # could be required, optional or user_prompt
     description: name of the project
   - key: language
     input_type: string
     requirement: required
     description: language of the code
   - key: complexity_threshold
     input_type: number
     requirement: optional
     default: 20 # default is required for optional parameters
     description: a threshold that defines the maximum allowed complexity
   - key: test_coverage
     input_type: number
     requirement: optional
     default: 80
     description: the minimum test coverage threshold in percentage
   - key: style_guide
     input_type: string
     description: style guide name
     requirement: user_prompt
     # If style_guide param value is not specified in the command, user will be prompted to provide a value, even in non-interactive mode
   ```
   </details>

   See the [Recipe Reference Guide](/docs/guides/recipes/recipe-reference) for more information about recipe fields. 

   </TabItem> 
</Tabs>

## Edit Recipe
<Tabs groupId="interface">
  <TabItem value="ui" label="goose Desktop" default>

   1. Click the <PanelLeft className="inline" size={16} /> button in the top-left to open the sidebar
   2. Click `Recipes` in the sidebar
   3. Find the recipe you want to edit and click the <SquarePen className="inline" size={16} /> button
   4. In the dialog that appears, edit any of the following:
      <RecipeFields />
   5. When you're finished, you can:
      - Copy the recipe link to share the recipe with others
      - Click `Save Recipe` to save your changes
      - Click `Save & Run Recipe` to save and immediately run the recipe in a new session

  :::tip Edit In-Use Recipe
  You can also access the edit dialog while using a recipe in a session: Just click the <ChefHat className="inline" size={16} /> button at the bottom of the app. The button shows up after you've sent your first message.
  :::
   
  </TabItem>

  <TabItem value="cli" label="goose CLI">
  Once the recipe file is created, you can open it with your preferred text editor and modify the value of any field.

</TabItem> 
</Tabs>

## Use Recipe

<Tabs groupId="interface">
  <TabItem value="ui" label="goose Desktop" default>

  1. Open the recipe using a direct link or manual URL entry, or from your Recipe library:

     **Direct Link:**

         1. Click a recipe link shared with you

     **Manual URL Entry:**

         1. Paste a recipe link into your browser's address bar 
         2. Press `Enter` and click the `Open Goose.app` prompt
       
     **Recipe Library:**

         1. Click the <PanelLeft className="inline" size={16} /> button in the top-left to open the sidebar
         2. Click `Recipes` in the sidebar
         3. Find your recipe in the Recipe Library
         4. Click `Use` next to the recipe you want to open

     **Slash Command:**

         1. Enter a [custom slash command](/docs/guides/context-engineering/slash-commands) in any goose chat session

  2. The first time you run a recipe, a warning dialog displays the recipe's title, description, and instructions for you to review. If you trust the recipe content, click `Trust and Execute` to continue. You won't be prompted again for the same recipe unless it changes.

  3. If the recipe contains parameters, enter your values in the `Recipe Parameters` dialog and click `Start Recipe`.
  
     Parameters are dynamic values used in the recipe:

     - **Required parameters** are marked with red asterisks (*)
     - **Optional parameters** show default values that can be changed

  4. The recipe automatically submits and goose begins execution. If the recipe includes a [prompt](#core-components), it's sent as the first message. If not, you can click an activity bubble or send a prompt to get started.

  :::info Privacy & Isolation
  - Each person gets their own private session
  - No data is shared between users
  - Your session won't affect the original recipe creator's session
  :::
  </TabItem>

  <TabItem value="cli" label="goose CLI">

  Using a recipe with the goose CLI might involve the following tasks:
  - [Configuring your recipe location](#configure-recipe-location)
  - [Running a recipe](#run-a-recipe)
  - [Scheduling a recipe](#schedule-recipe)

   #### Configure Recipe Location

  Recipes can be stored locally on your device or in a GitHub repository. Configure your recipe repository using either the `goose configure` command or [config file](/docs/guides/config-files#global-settings).

  :::tip Repository Structure
  - Each recipe should be in its own directory
  - Directory name matches the recipe name you use in commands
  - Recipe file can be either recipe.yaml or recipe.json
  :::

   <Tabs>
     <TabItem value="configure" label="Using goose configure" default>

       Run the configure command:
       ```sh
       goose configure
       ```

       You'll see the following prompts:

       ```sh
       ┌  goose-configure 
       │
       ◆  What would you like to configure?
       │  ○ Configure Providers 
       │  ○ Add Extension 
       │  ○ Toggle Extensions 
       │  ○ Remove Extension 
       // highlight-start
       │  ● goose settings (Set the goose mode, Tool Output, Tool Permissions, Experiment, goose recipe github repo and more)
       // highlight-end
       │
       ◇  What would you like to configure?
       │  goose settings 
       │
       ◆  What setting would you like to configure?
       │  ○ goose mode 
       │  ○ Tool Permission 
       │  ○ Tool Output 
       │  ○ Toggle Experiment 
       // highlight-start
       │  ● goose recipe github repo (goose will pull recipes from this repo if not found locally.)
       // highlight-end
       └  
       ┌  goose-configure 
       │
       ◇  What would you like to configure?
       │  goose settings 
       │
       ◇  What setting would you like to configure?
       │  goose recipe github repo 
       │
       ◆  Enter your goose recipe GitHub repo (owner/repo): eg: my_org/goose-recipes
       // highlight-start
       │  squareup/goose-recipes (default)
       // highlight-end
       └  
       ```

     </TabItem>

     <TabItem value="config" label="Using config file">

       Add to your config file:
       ```yaml title="~/.config/goose/config.yaml"
       GOOSE_RECIPE_GITHUB_REPO: "owner/repo"
       ```

     </TabItem>
   </Tabs>

   #### Run a Recipe

   <Tabs groupId="interface">
     <TabItem value="local" label="Local Recipe" default>

       **Basic Usage** - Run once and exit (see [run options](/docs/guides/goose-cli-commands#run-options) and [recipe commands](/docs/guides/goose-cli-commands#recipe) for more):
       ```sh
       # Using recipe file in current directory or [`GOOSE_RECIPE_PATH`](/docs/guides/environment-variables#recipe-configuration) directories
       goose run --recipe recipe.yaml

       # Using full path
       goose run --recipe ./recipes/my-recipe.yaml
       ```

       **Preview Recipe** - Use the [`explain`](/docs/guides/goose-cli-commands#run-options) command to view details before running:
 
       **Interactive Mode** - Start an interactive session:
       ```sh
       goose run --recipe recipe.yaml --interactive
       ```
       The interactive mode will prompt for required values:
       ```sh
       ◆ Enter value for required parameter 'language':
       │ Python
       │
       ◆ Enter value for required parameter 'style_guide':
       │ PEP8
       ```

       **With Parameters** - Supply parameter values when running recipes. See the [`run` command documentation](/docs/guides/goose-cli-commands#run-options) for detailed examples and options.

       Basic example:
       ```sh
       goose run --recipe recipe.yaml --params language=Python
       ```

       **Slash Command** - Enter a [custom slash command](/docs/guides/context-engineering/slash-commands) in any goose chat session

     </TabItem>

     <TabItem value="github" label="GitHub Recipe">

       Once you've configured your GitHub repository, you can run recipes by name:

       **Basic Usage** - Run recipes from your configured repo using the recipe name that matches its directory (see [run options](/docs/guides/goose-cli-commands#run-options) and [recipe commands](/docs/guides/goose-cli-commands#recipe) for more):

       ```sh
       goose run --recipe recipe-name
       ```

       For example, if your repository structure is:
       ```
       my-repo/
       ├── code-review/
       │   └── recipe.yaml
       └── setup-project/
           └── recipe.yaml
       ```
       
       You would run the following command to run the code review recipe:
       ```sh
       goose run --recipe code-review
       ```

      **Preview Recipe** - Use the [`explain`](/docs/guides/goose-cli-commands#run-options) command to view details before running:

       **Interactive Mode** - With parameter prompts:
       ```sh
       goose run --recipe code-review --interactive
       ```
       The interactive mode will prompt for required values:
       ```sh
       ◆ Enter value for required parameter 'project_name':
       │ MyProject
       │
       ◆ Enter value for required parameter 'language':
       │ Python
       ```

       **With Parameters** - Supply parameter values when running recipes. See the [`run` command documentation](/docs/guides/goose-cli-commands#run-options) for detailed examples and options.

     </TabItem>
   </Tabs>
  :::info Privacy & Isolation
  - Each person gets their own private session
  - No data is shared between users
  - Your session won't affect the original recipe creator's session
  :::

   </TabItem>
</Tabs>

## Validate Recipe

<Tabs groupId="interface">
  <TabItem value="ui" label="goose Desktop" default>
    Recipe validation is only available through the CLI.
  </TabItem>
  <TabItem value="cli" label="goose CLI">
    Validate your recipe file to ensure it's properly configured. Validation verifies that:
    - All required fields are present
    - Parameters are properly formatted
    - Referenced extensions exist and are valid
    - The YAML/JSON syntax is correct

   ```sh
   goose recipe validate recipe.yaml
   ```

   :::info
   If you want to validate a recipe you just created, you need to [exit the session](/docs/guides/sessions/session-management#exit-session) before running the [`validate` subcommand](/docs/guides/goose-cli-commands#recipe).
   :::

   Recipe validation can be useful for:
    - Troubleshooting recipes that aren't working as expected
    - Verifying recipes after manual edits
    - Automated testing in CI/CD pipelines

  </TabItem>
</Tabs>

## Share Recipe
Share your recipe with goose users using a recipe link or recipe file.

:::info Privacy & Isolation
Each recipient gets their own private session when using your shared recipe. No data is shared between users, and your original session and recipe remain unaffected.
:::

### Share via Recipe Link
You can share a recipe with Desktop users via a recipe link.

<Tabs groupId="interface">
  <TabItem value="ui" label="goose Desktop" default>
    Copy the deeplink from your Recipe Library to share with others:
    1. Click the <PanelLeft className="inline" size={16} /> button in the top-left to open the sidebar
    2. Click `Recipes` in the sidebar
    3. Find the recipe you want to share and click the <Link className="inline" size={16} /> button to copy the link

  </TabItem>
  <TabItem value="cli" label="goose CLI">
    Generate a deeplink from your recipe file to share with others:
    ```sh
    goose recipe deeplink <FILE>
    ```

    You can also provide parameter values to pre-fill the `Recipe Parameters` dialog:
    ```sh
    goose recipe deeplink <FILE> --param key1=value1 --param key2=value2
    ```
  </TabItem>
</Tabs>

When someone clicks the link, it will open goose Desktop with your recipe configuration. They can also use your recipe link to [import a recipe](/docs/guides/recipes/storing-recipes#importing-recipes) for future use.

### Share via Recipe File
You can share a recipe with Desktop or CLI users by sending the recipe file directly.

<Tabs groupId="interface">
  <TabItem value="ui" label="goose Desktop" default>

  In goose Desktop, you can export a recipe file or copy its content to share with others.

  1. Click the <PanelLeft className="inline" size={16} /> button in the top-left to open the sidebar
  2. Click `Recipes` in the sidebar
  3. Find the recipe you want to share and click the <Share2 className="inline" size={16} /> button
  4. Choose a sharing method:
     - To download the recipe as a `.yaml` file: Choose `Export to File`, select a download location, and click `Save`
     - To copy the recipe's YAML content to your clipboard: Choose `Copy YAML`

  Other Desktop users can [import the recipe](/docs/guides/recipes/storing-recipes#importing-recipes) to their Recipe Library.

  </TabItem>
  <TabItem value="cli" label="goose CLI">

  Exporting or copying recipe content is only available through the Desktop, but you can copy local recipe files directly.

  CLI users can run a shared recipe file using `goose run --recipe <FILE>` or open it directly in goose Desktop with `goose recipe open <FILE>`. See the [CLI Commands guide](/docs/guides/goose-cli-commands#recipe) for details.

  </TabItem>
</Tabs>

## Schedule Recipe
<Tabs groupId="interface">
  <TabItem value="ui" label="goose Desktop" default>
Automate goose recipes by running them on a schedule. When creating a schedule, you'll configure:
- **Name**: A descriptive name for the schedule
- **Source**: The recipe to run
- **Execution mode**: Whether the recipe runs in the background (no window, results saved) or foreground (opens window if goose Desktop is running, otherwise runs in background)
- **Frequency and time**: When to run the recipe (e.g. every 20 minutes, weekly at 10 AM on Friday). Your selection is converted into a [cron expression](https://en.wikipedia.org/wiki/Cron#Cron_expression) used by goose.

**Schedule from Recipe Library:**

   1. Click the <PanelLeft className="inline" size={16} /> button in the top-left to open the sidebar
   2. Click `Recipes` in the sidebar
   3. Find the recipe you want to schedule and click the <Clock className="inline" size={16} /> button
   4. Click `Create Schedule`
   5. In the dialog that appears, configure the schedule. For **Source**, your recipe link is already provided.
   6. Click `Create Schedule`

**Schedule from Scheduler View:**

   1. Click the <PanelLeft className="inline" size={16} /> button in the top-left to open the sidebar
   2. Click `Scheduler` 
   3. Click `Create Schedule`
   4. In the dialog that appears, configure the schedule. For **Source**, select a `.yaml` or `.yml` file or provide a [recipe link](#share-recipe).
   5. Click `Create Schedule`

**Manage Scheduled Recipes**

Your scheduled recipes are listed in the `Scheduler` page.
Click on a schedule to view details, see when it was last run, and perform actions with the scheduled recipe:
- `Run Schedule Now` to trigger the recipe manually
- `Edit Schedule` to change the scheduled frequency
- `Pause Schedule` to stop the recipe from running automatically

At the bottom of the `Schedule Details` page you can view the list of sessions created by the scheduled recipe and open or restore each session.

  </TabItem>
  <TabItem value="cli" label="goose CLI">
  Automate goose recipes by scheduling them to run with a [cron expression](https://en.wikipedia.org/wiki/Cron#Cron_expression).

  ```bash
  # Add a new scheduled recipe which runs every day at 9 AM
  goose schedule add --schedule-id daily-report --cron "0 0 9 * * *" --recipe-source ./recipes/daily-report.yaml
  ```
  You can use either a 5, 6, or 7-digit cron expression for full scheduling precision, following the format "seconds minutes hours day-of-month month day-of-week year".

  See the [`schedule` command documentation](/docs/guides/goose-cli-commands.md#schedule) for detailed examples and options.
</TabItem>
</Tabs>

## Core Components

 A recipe needs these core components:

   - **Instructions**: Define the agent's behavior and capabilities
      - Acts as the agent's mission statement
      - Makes the agent ready for any relevant task
      - Required if no prompt is provided

   - **Prompt** (Optional): Starts the conversation automatically
      - Without a prompt, the agent waits for user input
      - Useful for specific, immediate tasks
      - Required if no instructions are provided

   - **Activities**: Example tasks that appear as clickable bubbles
      - Help users understand what the recipe can do
      - Make it easy to get started

## Advanced Features

### Automated Retry Logic

Recipes can include retry logic to automatically attempt task completion multiple times until success criteria are met. This is particularly useful for:

- **Automation workflows** that need to ensure successful completion
- **Development tasks** like running tests that may need multiple attempts  
- **System operations** that require validation and cleanup

**Basic retry configuration:**
```yaml
retry:
  max_retries: 3
  checks:
    - type: shell
      command: "test -f output.txt"  # Check if output file exists
  on_failure: "rm -f temp_files*"   # Cleanup on failure
```

**How it works:**
1. Recipe executes normally with provided instructions
2. After completion, success checks validate the results
3. If validation fails and retries remain:
   - Optional cleanup command runs
   - Agent state resets to initial conditions
   - Recipe execution starts over
4. Process continues until either success or max retries reached

See the [Recipe Reference Guide](/docs/guides/recipes/recipe-reference#retry) for complete retry configuration options and examples.

### Structured Output for Automation

Recipes can enforce [structured JSON output](/docs/guides/recipes/recipe-reference#response), making them ideal for automation workflows that need to parse and process agent responses reliably. Key benefits include:

- **Reliable parsing**: Consistent JSON format for scripts, automation, and CI/CD pipelines
- **Built-in validation**: Ensures output matches your requirements  
- **Easy extraction**: Final output appears as a single line for simple parsing

Structured output is particularly useful for: 
- **Development workflows**: Code analysis reports, test results with pass/fail counts, and build status with deployment readiness
- **Data processing**: Results with counts and validation status, content analysis with structured findings  
- **Documentation generation**: Consistent metadata and structured project reports for further processing

**Example structured output configuration:**
```yaml
response:
  json_schema:
    type: object
    properties:
      build_status:
        type: string
        enum: ["success", "failed", "warning"]
        description: "Overall build result"
      tests_passed:
        type: number
        description: "Number of tests that passed"
      tests_failed:
        type: number
        description: "Number of tests that failed"
      artifacts:
        type: array
        items:
          type: string
        description: "Generated build artifacts"
      deployment_ready:
        type: boolean
        description: "Whether the build is ready for deployment"
    required:
      - build_status
      - tests_passed
      - tests_failed
      - deployment_ready
```

**How it works:**
1. Recipe runs normally with provided instructions
2. goose calls a `final_output` tool with JSON matching your schema
3. Output is validated against the JSON schema
4. If validation fails, goose receives error details and must correct the output
5. Final validated JSON appears as the last line of output for easy extraction

**Example automation usage:**
```bash
# Run recipe and extract JSON output
goose run --recipe analysis.yaml --params project_path=./src > output.log
RESULT=$(tail -n 1 output.log)
echo "Analysis Status: $(echo $RESULT | jq -r '.build_status')"
echo "Issues Found: $(echo $RESULT | jq -r '.tests_failed')"
```

:::info
Structured output is supported in recipes run in both the goose CLI and goose Desktop. However, creating and editing the `json_schema` configuration must be done manually in the recipe file.
:::

## What's Included

A recipe captures:

- AI instructions (goal/purpose)  
- Suggested activities (examples for the user to click)  
- Enabled extensions and their configurations  
- Project folder or file context  
- Initial setup (but not full conversation history)
- The model and provider to use when running the recipe (optional)
- Retry logic and success validation configuration (if configured)


To protect your privacy and system integrity, goose excludes:

- Global and local memory  
- API keys and personal credentials  
- System-level goose settings  


This means others may need to supply their own credentials or memory context if the recipe depends on those elements.

## Learn More
Check out the [Recipes](/docs/guides/recipes) guide for more docs, tools, and resources to help you master goose recipes.
