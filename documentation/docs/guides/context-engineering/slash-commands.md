---
sidebar_position: 4
title: Custom Slash Commands
sidebar_title: Slash Commands
description: "Create custom shortcuts to quickly apply reusable instructions in any goose chat session"
---

import Tabs from '@theme/Tabs';
import TabItem from '@theme/TabItem';
import { PanelLeft, Terminal } from 'lucide-react';

Custom slash commands are personalized shortcuts to run [recipes](/docs/guides/recipes). If you have a recipe that runs a daily report, you can create a custom slash command to invoke that recipe from within a session:

```
/daily-report
```


## Create Slash Commands

Assign a custom command to a recipe.

<Tabs groupId="interface">
  <TabItem value="ui" label="goose Desktop" default>
   1. Click the <PanelLeft className="inline" size={16} /> button in the top-left to open the sidebar
   2. Click `Recipes` in the sidebar
   3. Find the recipe you want to use and click the <Terminal className="inline" size={16} /> button
   4. In the modal that pops up, type your custom command (without the leading `/`)
   5. Click `Save`
 
  The command appears under the recipe in your `Recipes` menu. For recipes that aren't in your Recipe Library, follow the `goose CLI` steps.

  </TabItem>
  <TabItem value="cli" label="goose CLI">

  Configure slash commands in your [configuration file](/docs/guides/config-files). List the command (without the leading `/`) along with the path to the recipe file on your computer:

```yaml title="~/.config/goose/config.yaml"
slash_commands:
  - command: "run-tests"
    recipe_path: "/path/to/recipe.yaml"
  - command: "daily-report"
    recipe_path: "/Users/me/.local/share/goose/recipes/report.yaml"
```

   </TabItem>
</Tabs>

## Use Slash Commands

In any chat session, type your custom command with a leading slash at the start of your message:

<Tabs groupId="interface">
  <TabItem value="ui" label="goose Desktop" default>

```
/run-tests
```

:::tip Available Commands
Typing `/` in goose Desktop shows a popup menu with the available slash commands.
:::

  </TabItem>
  <TabItem value="cli" label="goose CLI">

```sh
Context: ●○○○○○○○○○ 5% (9695/200000 tokens)
( O)> /run-tests
```

  </TabItem>
</Tabs>

You can pass one parameter after the command (if needed). Quotation marks are optional:

```
/translator where is the library
```

When you run a recipe using a slash command, the recipe's instructions and prompt fields are sent to your model and loaded into the conversation, but not displayed in chat. The model responds using the recipe's context and instructions just as if you opened it directly.

## Limitations

- Slash commands accept only one [parameter](/docs/guides/recipes/recipe-reference#parameters). Any additional parameters in the recipe must have default values.
- Command names are case-insensitive (`/Bug` and `/bug` are treated as the same command).
- Command names must be unique and contain no spaces.
- You cannot use names that conflict with [built-in CLI slash commands](/docs/guides/goose-cli-commands#slash-commands) like `/compact` or `/help`.
- If the recipe file is missing or invalid, the command will be treated as regular text sent to the model.

## Additional Resources

import ContentCardCarousel from '@site/src/components/ContentCardCarousel';

<ContentCardCarousel
  items={[
    {
      type: 'topic',
      title: 'Recipes',
      description: 'Check out the Recipes guide for more docs, tools, and resources to help you master goose recipes.',
      linkUrl: '/docs/guides/recipes'
    },
    {
      type: 'topic',
      title: 'Research → Plan → Implement Patterns',
      description: 'See how slash commands make it easy to integrate instructions into interactive RPI workflows.',
      linkUrl: '/docs/tutorials/rpi'
    }
  ]}
/>
