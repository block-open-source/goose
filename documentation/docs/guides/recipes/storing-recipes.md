---
title: Saving Recipes
sidebar_position: 4
sidebar_label: Saving Recipes
---

import Tabs from '@theme/Tabs';
import TabItem from '@theme/TabItem';
import { PanelLeft, ChefHat } from 'lucide-react';

This guide covers storing, organizing, and finding goose recipes when you need to access them again later. 

:::info Desktop UI vs CLI
- **goose Desktop** has a visual Recipe Library for browsing and managing saved recipes
- **goose CLI** stores recipes as files that you find using file paths or environment variables
:::

## Understanding Recipe Storage

Before saving recipes, it's important to understand where they can be stored and how this affects their availability.

### Recipe Storage Locations

| Type | Location | Availability | Best For |
|------|----------|-------------|----------|
| **Global** | `~/.config/goose/recipes/` | All projects and sessions | Personal workflows, general-purpose recipes |
| **Local** | `YOUR_WORKING_DIRECTORY/.goose/recipes/` | Only when working in that project | Project-specific workflows, team recipes |

**Choose Global Storage When:**
- You want the recipe available across all projects
- It's a personal workflow or general-purpose recipe
- You're the primary user of the recipe

**Choose Local Storage When:**
- The recipe is specific to a particular project
- You're working with a team and want to share the recipe
- The recipe depends on project-specific files or configurations


## Storing Recipes

<Tabs groupId="interface">
  <TabItem value="desktop" label="goose Desktop" default>

**Save New Recipe:**

1. Open `Recipes` from the sidebar and click `Create Recipe`
2. Complete the Recipe Editor, then click `Save Recipe` to save it to your Recipe Library

**Save Modified Recipe:**

If you're already using a recipe and want to save a modified version:
1. Click the <ChefHat className="inline" size={16}/> button at the bottom of the app, which appears after sending your first message
2. Make any desired edits to the instructions, prompt, or other fields
3. Click `Save Recipe`

:::info
When you modify and save a recipe with a new name, a new recipe and new link are generated. You can still run the original recipe from the recipe library, or using the original link. If you edit a recipe without changing its name, the version in the recipe library is updated, but you can still run the original recipe via link.
:::

  </TabItem>
  <TabItem value="cli" label="goose CLI">

    Create recipe files with your preferred editor. Store project-specific recipes in `.goose/recipes/` and global recipes in `~/.config/goose/recipes/`. The CLI can run recipes in either YAML or JSON format.

  </TabItem>
</Tabs>

### Importing Recipes

<Tabs groupId="interface">
  <TabItem value="desktop" label="goose Desktop" default>
    Import a recipe using its deeplink or recipe file:

    1. Click the <PanelLeft className="inline" size={16} /> button in the top-left to open the sidebar
    2. Click `Recipes` in the sidebar
    3. Click `Import Recipe`
    4. Choose your import method:
       - To import via a link: Under `Recipe Deeplink`, paste in the [recipe link](/docs/guides/recipes/session-recipes#share-via-recipe-link)
       - To import via a file: Under `Recipe File`, click `Choose File`, select a recipe file, and click `Open`
    5. Click `Import Recipe` to save a copy of the recipe to your Recipe Library

  :::warning Recipe File Format
  goose Desktop accepts `.yaml`, `.yml`, and `.json` files, but **the CLI only supports `.yaml` and `.json`**. For full compatibility across both interfaces, avoid `.yml` extensions.

  All recipe formats follow the same [schema structure](/docs/guides/recipes/recipe-reference#core-recipe-schema).
  :::

  </TabItem>
  <TabItem value="cli" label="goose CLI">
    Recipe import is only available in goose Desktop.
  </TabItem>
</Tabs>

## Finding Available Recipes

<Tabs groupId="interface">
  <TabItem value="desktop" label="goose Desktop" default>

**Access Recipe Library:**
1. Click the <PanelLeft className="inline" size={16} /> button in the top-left to open the sidebar
2. Click `Recipes` to view your Recipe Library
3. Browse your available recipes, which show:
   - Recipe title and description
   - Last modified date
   - Whether they're stored globally or locally

:::info Desktop vs CLI Recipe Discovery
The Desktop Recipe Library displays all recipes you've explicitly saved or imported. It doesn't automatically discover recipe files from your filesystem like the CLI does.
:::

  </TabItem>
  <TabItem value="cli" label="goose CLI">

Use the `goose recipe list` command to find all available recipes from multiple sources:

**Basic Usage**

```bash
# List all available recipes
goose recipe list

# Show detailed information including titles and full paths
goose recipe list --verbose

# Output in JSON format for automation
goose recipe list --format json
```

**Recipe Discovery Process**

goose searches for recipes in the following locations (in order):

1. **Current directory**: `.` (looks for `*.yaml` and `*.json` files)
2. **Custom paths**: Directories specified in [`GOOSE_RECIPE_PATH`](/docs/guides/environment-variables#recipe-configuration) environment variable
3. **Global recipe library**: `~/.config/goose/recipes/` (or equivalent on your OS)
4. **Local project recipes**: `./.goose/recipes/`
5. **GitHub repository**: If [`GOOSE_RECIPE_GITHUB_REPO`](/docs/guides/environment-variables#recipe-configuration) environment variable is configured

**Example Output**

*Default text format:*
```bash
$ goose recipe list
Available recipes:
goose-self-test - A comprehensive meta-testing recipe - local: ./goose-self-test.yaml
hello-world - A sample recipe demonstrating basic usage - local: ~/.config/goose/recipes/hello-world.yaml
job-finder - Find software engineering positions - local: ~/.config/goose/recipes/job-finder.yaml
```

*Verbose mode:*
```bash
$ goose recipe list --verbose
Available recipes:
  goose-self-test - A comprehensive meta-testing recipe - local: ./goose-self-test.yaml
    Title: goose Self-Testing Integration Suite
    Path: ./goose-self-test.yaml
  hello-world - A sample recipe demonstrating basic usage - local: ~/.config/goose/recipes/hello-world.yaml
    Title: Hello World Recipe
    Path: /Users/username/.config/goose/recipes/hello-world.yaml
```

*JSON format for automation:*
```json
[
  {
    "name": "goose-self-test",
    "source": "Local",
    "path": "./goose-self-test.yaml",
    "title": "goose Self-Testing Integration Suite",
    "description": "A comprehensive meta-testing recipe"
  },
  {
    "name": "hello-world",
    "source": "GitHub",
    "path": "recipes/hello-world.yaml",
    "title": "Hello World Recipe",
    "description": "A sample recipe demonstrating basic usage"
  }
]
```

**Configuring Recipe Sources**

Add custom recipe directories:
```bash
export GOOSE_RECIPE_PATH="/path/to/my/recipes:/path/to/team/recipes"
goose recipe list
```

Configure GitHub recipe repository:
```bash
export GOOSE_RECIPE_GITHUB_REPO="myorg/goose-recipes"
goose recipe list
```

See the [Environment Variables Guide](/docs/guides/environment-variables#recipe-configuration) for more configuration options.

**Manual Directory Browsing (Advanced)**

If you need to browse recipe directories manually:

```bash
# List recipes in default global location
ls ~/.config/goose/recipes/

# List recipes in current project
ls .goose/recipes/

# Search for all recipe files
find . -name "*.yaml" -path "*/recipes/*" -o -name "*.json" -path "*/recipes/*"
```

:::tip
The `goose recipe list` command is the recommended way to find recipes as it automatically searches all configured sources and provides consistent formatting.
:::

  </TabItem>
</Tabs>

## Using Saved Recipes

<Tabs groupId="interface">
  <TabItem value="desktop" label="goose Desktop" default>

1. Click the <PanelLeft className="inline" size={16} /> button in the top-left to open the sidebar
2. Click `Recipes`
3. Find your recipe in the Recipe Library
4. Choose one of the following:
   - Click `Use` to run it immediately
   - Click `Preview` to see the recipe details first, then click **Load Recipe** to run it

  </TabItem>
  <TabItem value="cli" label="goose CLI">

Once you've located your recipe file, [run the recipe](/docs/guides/recipes/session-recipes#run-a-recipe) or [open it in goose Desktop](/docs/guides/goose-cli-commands#recipe).

:::tip Format Compatibility
The CLI can run recipes saved from goose Desktop without any conversion. Both CLI-created and Desktop-saved recipes work with all recipe commands.
:::

  </TabItem>
</Tabs>
