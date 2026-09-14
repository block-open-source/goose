---
title: Quick goose Tips
sidebar_position: 30
sidebar_label: Quick Tips
description: Best practices for working with goose
---

### goose works on your behalf
goose is an AI agent, which means you can prompt goose to perform tasks for you like opening applications, running shell commands, automating workflows, writing code, browsing the web, and more.

### Prompt goose using natural language
You don't need fancy language or special syntax to prompt goose. Talk with goose like you would talk to a friend. You can even use slang or say please and thank you; goose will understand.

### Extend goose's capabilities to any application
goose's capabilities are extensible. As an [MCP](https://modelcontextprotocol.io/) client, goose can connect to your apps and services through [extensions](/extensions), allowing it to work across your entire workflow.

### Choose how much control goose has
You can customize how much [supervision](/docs/guides/managing-tools/goose-permissions) goose needs. Choose between full autonomy, requiring approval before actions, or simply chatting without any actions.

### Choose the right LLM
Your experience with goose is shaped by your [choice of LLM](/blog/2025/03/31/goose-benchmark), as it handles all the planning while goose manages the execution. When choosing an LLM, consider its tool support, specific capabilities, and associated costs.

### Keep sessions short
LLMs have context windows, which are limits on how much conversation history they can retain. Once exceeded, they may forget earlier parts of the conversation. Monitor your token usage and [start new sessions](/docs/guides/sessions/session-management) as needed.

### Use Quick Launcher for faster session starts
Press `Cmd+Option+Shift+G` (macOS) or `Ctrl+Alt+Shift+G` (Windows/Linux) and send a prompt to start a new session instantly.

### Turn off unnecessary extensions or tool
Turning on too many extensions can degrade performance. Enable only essential [extensions and tools](/docs/guides/managing-tools/tool-permissions) to improve tool selection accuracy, save context window space, and stay within provider tool limits.

:::tip Code Mode for Many Extensions
Consider enabling [Code Mode](/docs/guides/managing-tools/code-mode), an alternative approach to tool calling that discovers tools on demand.
:::

### Teach goose your preferences
Help goose remember how you like to work by using [`.goosehints` or other context files](/docs/guides/context-engineering/using-goosehints) or [skills](/docs/guides/context-engineering/using-skills) for permanent project preferences and the [Memory extension](/docs/mcp/memory-mcp) for things you want goose to dynamically recall later. Both can help save valuable context window space while keeping your preferences available.

### Protect sensitive files
Use [permission modes](/docs/guides/managing-tools/goose-permissions) and [tool permissions](/docs/guides/managing-tools/tool-permissions) when working around files you do not want goose to change.

### Version Control
Commit your code changes early and often. This allows you to rollback any unexpected changes.

### Control which extensions goose can use
Administrators can use an [allowlist](/docs/guides/allowlist) to restrict goose to approved extensions only. This helps prevent risky installs from unknown MCP servers.

### Set up starter templates
Create a reusable [recipe](/docs/guides/recipes/session-recipes) for workflows you want to share or run again later.

### Embrace an experimental mindset
You don’t need to get it right the first time. Iterating on prompts and tools is part of the workflow.

### Customize the sidebar
goose Desktop lets you [customize the sidebar](/docs/guides/desktop-navigation) to match how you like to work. Adjust its position, appearance, and which items are visible.

### Keep goose updated
Regularly [update](/docs/guides/updating-goose) goose to benefit from the latest features, bug fixes, and performance improvements.

### Use a Dedicated Planner Model
Use [planning mode](/docs/guides/context-engineering/creating-plans) with a dedicated planner model for complex reasoning, while keeping a faster default model for everyday execution.

### Make Recipes Safe to Re-run
Write [recipes](/docs/guides/recipes/session-recipes) that check your current state before acting, so they can be run multiple times without causing any errors or duplication. 

### Add Logging to Recipes
Include informative log messages in your recipes for each major step to make debugging and troubleshooting easier should something fail.
