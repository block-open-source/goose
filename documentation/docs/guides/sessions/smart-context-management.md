---
title: Smart Context Management
sidebar_position: 3
sidebar_label: Smart Context Management
---

import Tabs from '@theme/Tabs';
import TabItem from '@theme/TabItem';
import { ScrollText } from 'lucide-react';
import { PanelLeft } from 'lucide-react';

When working with [Large Language Models (LLMs)](/docs/getting-started/providers), there are limits to how much conversation history they can process at once. goose provides smart context management features to help handle context and conversation limits so you can maintain productive sessions. Here are some key concepts:

- **Context Length**: The amount of conversation history the LLM can consider, also referred to as the context window
- **Context Limit**: The maximum number of tokens the model can process
- **Context Management**: How goose handles conversations approaching these limits
- **Turn**: One complete prompt-response interaction between goose and the LLM

## How goose Manages Context
goose uses a two-tiered approach to context management:

1. **Auto-Compaction**: Proactively compacts conversation history when approaching token limits
2. **Context Strategies**: Backup strategy used if the context limit is still exceeded after auto-compaction

This layered approach lets goose handle token and context limits gracefully.

## Automatic Compaction
goose automatically compacts older parts of your conversation into a summary when approaching token limits, allowing you to maintain long-running sessions without manual intervention.
Auto-compaction is triggered by default when you reach 80% of the token limit in goose Desktop and the goose CLI.

Control the auto-compaction behavior with the `GOOSE_AUTO_COMPACT_THRESHOLD` [environment variable](/docs/guides/environment-variables.md#session-management). 
Disable this feature by setting the value to `0.0`.

```
# Automatically compact sessions when 60% of available tokens are used
export GOOSE_AUTO_COMPACT_THRESHOLD=0.6
```

When you reach the auto-compaction threshold:
  1. goose will automatically start compacting the conversation to make room.
  2. Once complete, you'll see a confirmation message that the conversation was compacted and summarized.
  3. Continue the session. Your previous conversation remains visible, but only the compacted conversation is included in the active context for goose.

:::tip Customize Compaction
You can customize how goose summarizes conversations during compaction by editing the `compaction.md` [prompt template](/docs/guides/context-engineering/prompt-templates).
:::

:::tip Tool Output Summarization
To help maintain efficient context usage, goose summarizes older tool call outputs in the background while keeping recent calls in full detail. By default, goose computes the cutoff from the model context limit and auto-compaction threshold. For advanced tuning, see [`GOOSE_TOOL_CALL_CUTOFF`](/docs/guides/environment-variables#session-management).
:::

### Manual Compaction
You can also trigger compaction manually before reaching context or token limits:

<Tabs groupId="interface">
  <TabItem value="ui" label="goose Desktop" default>

  1. Point to the token usage indicator dot next to the model name at the bottom of the app
  2. Click <ScrollText className="inline" size={16} /> `Compact now` in the context window that appears
  3. Once complete, you'll see a confirmation message that the conversation was compacted and summarized.
  4. Continue the session. Your previous conversation remains visible, but only the compacted conversation is included in the active context for goose.

  :::info 
  You must send at least one message in the chat before the `Compact now` button is enabled. 
  :::

</TabItem>
<TabItem value="cli" label="goose CLI" default>

To proactively compact the conversation before reaching context limits, use the `/compact` command:

```sh
( O)> /compact
◇  Are you sure you want to compact this conversation? This will condense the message history.
│  Yes 
│
Compacting conversation...
Conversation has been compacted.
Key information has been preserved while reducing context length.
```

`/summarize` still works as a deprecated alias for `/compact`, but `/compact` is the command to use in new docs and workflows.

</TabItem>
</Tabs>

## Context Limit Strategies

When auto-compaction is disabled, or if a conversation still exceeds the context limit, goose offers different ways to handle it:

| Feature | Description | Best For | Availability | Impact |
|---------|-------------|-----------|-----------|---------|
| **Summarization** | Condenses conversation while preserving key points | Long, complex conversations | Desktop and CLI | Maintains most context |
| **Truncation** | Removes oldest messages to make room | Simple, linear conversations | CLI only | Loses old context |
| **Clear** | Starts fresh while keeping session active | New direction in conversation | CLI only | Loses all context |
| **Prompt** | Asks user to choose from the above options | Control over each decision in interactive sessions | CLI only | Depends on choice made |

<Tabs groupId="interface">
  <TabItem value="ui" label="goose Desktop" default>

goose Desktop exclusively uses summarization by compacting the conversation to manage context, preserving key information while reducing size.

  </TabItem>
  <TabItem value="cli" label="goose CLI">

The CLI supports all context limit strategies: `summarize`, `truncate`, `clear`, and `prompt`. 

The default behavior depends on the mode you're running in:
- **Interactive mode**: Prompts user to choose (equivalent to `prompt`)
- **Headless mode** (`goose run`): Automatically summarizes (equivalent to `summarize`)

You can configure how goose handles context limits by setting the `GOOSE_CONTEXT_STRATEGY` environment variable:

```bash
# Set automatic strategy (choose one)
export GOOSE_CONTEXT_STRATEGY=summarize  # Automatically summarize (recommended)
export GOOSE_CONTEXT_STRATEGY=truncate   # Automatically remove oldest messages
export GOOSE_CONTEXT_STRATEGY=clear      # Automatically clear session

# Set to prompt the user
export GOOSE_CONTEXT_STRATEGY=prompt
```

When you hit the context limit, the behavior depends on your configuration:

**With default settings (no `GOOSE_CONTEXT_STRATEGY` set)**, you'll see this prompt to choose a management option:

```sh
◇  The model's context length is maxed out. You will need to reduce the # msgs. Do you want to?
│  ○ Clear Session   
│  ○ Truncate Message
// highlight-start
│  ● Summarize Session
// highlight-end

final_summary: [A summary of your conversation will appear here]

Context maxed out
--------------------------------------------------
goose summarized messages for you.
```

**With `GOOSE_CONTEXT_STRATEGY` configured**, goose will automatically apply your chosen strategy:

```sh
# Example with GOOSE_CONTEXT_STRATEGY=summarize
Context maxed out - automatically summarized messages.
--------------------------------------------------
goose automatically summarized messages for you.

# Example with GOOSE_CONTEXT_STRATEGY=truncate
Context maxed out - automatically truncated messages.
--------------------------------------------------
goose tried its best to truncate messages for you.

# Example with GOOSE_CONTEXT_STRATEGY=clear
Context maxed out - automatically cleared session.
--------------------------------------------------
```
  </TabItem>
</Tabs>

## Maximum Turns
The `Max Turns` limit is the maximum number of consecutive turns that goose can take without user input (default: 1000). When the limit is reached, goose stops and prompts: "I've reached the maximum number of actions I can do without user input. Would you like me to continue?" If the user answers in the affirmative, goose continues until the limit is reached and then prompts again.

This feature gives you control over agent autonomy and prevents infinite loops and runaway behavior, which could have significant cost consequences or damaging impact in production environments. Use it for:

- Preventing infinite loops and excessive API calls or resource consumption in automated tasks
- Enabling human supervision or interaction during autonomous operations
- Controlling loops while testing and debugging agent behavior

This setting is stored as the `GOOSE_MAX_TURNS` environment variable in your [config.yaml file](/docs/guides/config-files). You can configure it using the Desktop app or CLI.

<Tabs groupId="interface">
    <TabItem value="ui" label="goose Desktop" default>

      1. Click the <PanelLeft className="inline" size={16} /> button in the top-left to open the sidebar
      2. Click the `Settings` button on the sidebar
      3. Click the `Chat` tab 
      4. Scroll to `Conversation Limits` and enter a value for `Max Turns`
        
    </TabItem>
    <TabItem value="cli" label="goose CLI">

      1. Run the `configuration` command:
      ```sh
      goose configure
      ```

      2. Select `goose settings`:
      ```sh
      ┌   goose-configure
      │
      ◆  What would you like to configure?
      │  ○ Configure Providers
      │  ○ Add Extension
      │  ○ Toggle Extensions
      │  ○ Remove Extension
      // highlight-start
      │  ● goose settings (Set the goose mode, Tool Output, Tool Permissions, Experiment, goose recipe github repo and more)
      // highlight-end
      └ 
      ```

      3. Select `Max Turns`:
      ```sh
      ┌   goose-configure
      │
      ◇  What would you like to configure?
      │  goose settings
      │
      ◆  What setting would you like to configure?
      │  ○ goose mode 
      │  ○ Router Tool Selection Strategy 
      │  ○ Tool Permission 
      │  ○ Tool Output 
      // highlight-start
      │  ● Max Turns (Set maximum number of turns without user input)
      // highlight-end
      │  ○ Toggle Experiment 
      │  ○ goose recipe github repo 
      │  ○ Scheduler Type 
      └ 
      ```

      4. Enter the maximum number of turns:
      ```sh
      ┌   goose-configure 
      │
      ◇  What would you like to configure?
      │  goose settings 
      │
      ◇  What setting would you like to configure?
      │  Max Turns 
      │
        // highlight-start
      ◆  Set maximum number of agent turns without user input:
      │  10
        // highlight-end
      │
      └  Set maximum turns to 10 - goose will ask for input after 10 consecutive actions
      ```

      :::tip
      In addition to the persistent `Max Turns` setting, you can provide a runtime override for a specific session or task via the `goose session --max-turns` and `goose run --max-turns` [CLI commands](/docs/guides/goose-cli-commands).
      :::

    </TabItem>
    
</Tabs>

**Choosing the Right Value**

The appropriate max turns value depends on your use case and comfort level with automation:

- **5-10 turns**: Good for exploratory tasks, debugging, or when you want frequent check-ins. For example, "analyze this codebase and suggest improvements" where you want to review each step
- **25-50 turns**: Effective for well-defined tasks with moderate complexity, such as "refactor this module to use the new API" or "set up a basic CI/CD pipeline"
- **100+ turns**: More suitable for complex, multi-step automation where you trust goose to work independently, like "migrate this entire project from React 16 to React 18" or "implement comprehensive test coverage for this service"

Remember that even simple-seeming tasks often require multiple turns. For example, asking goose to "fix the failing tests" might involve analyzing test output (1 turn), identifying the root cause (1 turn), making code changes (1 turn), and verifying the fix (1 turn).

## Token Usage
After sending your first message, goose Desktop and goose CLI display token usage.

<Tabs groupId="interface">
    <TabItem value="ui" label="goose Desktop" default>
    The Desktop displays a colored circle next to the model name at the bottom of the session window. The color provides a visual indicator of your token usage for the session. 
      - **Green**: Normal usage - Plenty of context space available
      - **Orange**: Warning state - Approaching limit (80% of capacity)
      - **Red**: Error state - Context limit reached
    
    Hover over this circle to display:
      - The number of tokens used
      - The percentage of available tokens used
      - The total available tokens
      - A progress bar showing your current token usage
        
    </TabItem>
    <TabItem value="cli" label="goose CLI">
    The CLI displays a context label above each command prompt, showing:
      - A visual indicator using dots (●○) and colors to represent your token usage:
        - **Green**: Below 50% usage
        - **Yellow**: Between 50-85% usage
        - **Red**: Above 85% usage
      - Usage percentage
      - Current token count and context limit

    </TabItem>
</Tabs>

## Model Context Limit Overrides

Context limits are automatically detected based on your model name, but goose provides settings to override the default limits:

| Model | Description | Best For | Setting |
|-------|-------------|----------|---------|
| **Main** | Set context limit for the main model (also serves as fallback for other models) | LiteLLM proxies, custom models with non-standard names | `GOOSE_CONTEXT_LIMIT` |

:::info
This setting only affects the displayed token usage and progress indicators. Actual context management is handled by your LLM, so you may experience more or less usage than the limit you set, regardless of what the display shows.
:::

This feature is particularly useful with:

- **LiteLLM Proxy Models**: When using LiteLLM with custom model names that don't match goose's patterns
- **Enterprise Deployments**: Custom model deployments with non-standard naming  
- **Fine-tuned Models**: Custom models with different context limits than their base versions
- **Development/Testing**: Temporarily adjusting context limits for testing purposes

goose resolves context limits with the following precedence (highest to lowest):

1. Global environment variable (`GOOSE_CONTEXT_LIMIT`)
2. Explicit declarative or custom provider model configuration
3. Provider runtime discovery
4. Canonical model metadata
5. Global default (128,000 tokens)

**Configuration**

<Tabs groupId="interface">
  <TabItem value="ui" label="goose Desktop" default>

     Model context limit overrides are not yet available in the goose Desktop app.

  </TabItem>
  <TabItem value="cli" label="goose CLI">

    Context limit overrides only work as [environment variables](/docs/guides/environment-variables#model-context-limit-overrides), not in the config file.

    ```bash
    export GOOSE_CONTEXT_LIMIT=1000
    goose session
    ```

  </TabItem>
    
</Tabs>

**Scenarios**

1. LiteLLM proxy with custom model name

```bash
# LiteLLM proxy with custom model name
export GOOSE_PROVIDER="openai"
export GOOSE_MODEL="my-custom-gpt4-proxy"
export GOOSE_CONTEXT_LIMIT=200000  # Override the 32k default
```

## Credit Balance Monitoring

goose monitors your API provider balance and warns you when credits are running low or exhausted. When this happens, you'll see an **Insufficient Credits** notification.

For providers that support it (such as [Tetrate Agent Router Service](https://router.tetrate.ai)), the notification includes an **Add credits** button that takes you directly to your provider's billing page.

**What to do:**
1. Click the **Add credits** button (if available) to top up your account
2. Or visit your provider's dashboard manually to add credits
3. Once credits are added, resend your message to continue the conversation

:::tip
goose detects low balance conditions automatically, so you won't lose your conversation context—just add credits and pick up where you left off.
:::

**Supported providers:** Tetrate Agent Router Service, OpenRouter, and other providers that report balance information via HTTP 402 responses.

## Cost Tracking
Display real-time estimated costs of your session.

<Tabs groupId="interface">
    <TabItem value="ui" label="goose Desktop" default>
To manage live cost tracking:
  1. Click the <PanelLeft className="inline" size={16} /> button in the top-left to open the sidebar
  2. Click the `Settings` button on the sidebar
  3. Click the `App` tab 
  4. Toggle `Cost Tracking` on/off

The session cost is shown at the bottom of the goose window and updates dynamically as tokens are consumed. Hover over the cost to see a detailed breakdown of token usage. If multiple models are used in the session, this includes a cost breakdown by model. Ollama and local deployments always show a cost of $0.00.

Pricing data is regularly fetched from the OpenRouter API and cached locally. The `Advanced settings` tab shows when the data was last updated and allows you to refresh. 

These costs are estimates only, and not connected to your actual provider bill. The cost shown is an approximation based on token counts and public pricing data.
</TabItem>
    <TabItem value="cli" label="goose CLI">
    Show estimated cost in the goose CLI by setting the `GOOSE_CLI_SHOW_COST` [environment variable](/docs/guides/environment-variables.md#session-management) or including it in the [configuration file](/docs/guides/config-files.md).

  ```
  # Set environment variable
  export GOOSE_CLI_SHOW_COST=true

  # config.yaml
  GOOSE_CLI_SHOW_COST: true
  ```
  </TabItem>
</Tabs>
