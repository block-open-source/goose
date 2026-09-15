---
title: "8 Things You Didn't Know About Code Mode"
description: Discover how Code mode reduces context rot and token usage in AI agents making them more efficient for long running sessions.
image: /img/blog/code-mode-header-image.png
authors:
    - rizel
---

![blog cover](/img/blog/code-mode-header-image.png)

Agents fundamentally changed how we program. They enable developers to move faster by disintermediating the traditional development workflow. This means less time switching between specialized tools and fewer dependencies on other teams. Now that agents can execute complicated tasks, developers face a new challenge: using them effectively over long sessions.

The biggest challenge is context rot. Because agents have limited memory, a session that runs too long can cause them to "forget" earlier instructions. This leads to unreliable outputs, frustration, and subtle but grave mistakes in your codebase. One promising solution is Code Mode. 

<!-- truncate -->

Instead of describing dozens of separate tools to an LLM, Code Mode allows an agent to write code that calls those tools programmatically, reducing the amount of context the model has to hold at once. While many developers first heard about Code Mode through [Cloudflare's blog post](https://blog.cloudflare.com/code-mode/), fewer understand how it works in practice. 

I have been using Code Mode for a few months and recently ran a small experiment. I asked goose to fix its own bug where the Gemini model failed to process images in the CLI but worked in the desktop app, then open a PR. The fix involved analyzing model configuration, tracing image input handling through the pipeline, and validating behavior across repeated runs. I ran the same task twice: once with Code Mode enabled and once without it.

Here is what I learned from daily use and my experiment.

## 1. Code Mode is Not an MCP-Killer

In fact, it uses MCP under the hood. MCP is a standard that lets AI agents connect to external tools and data sources. When you install an MCP server in an agent, that MCP server exposes its capabilities as MCP tools. For example, goose's primary MCP server called the `developer` extension exposes tools like `shell` enabling goose to run commands and `text_editor`, so goose can view and edit files. 

Code Mode wraps your MCP tools as JavaScript modules, allowing the agent to combine multiple tool calls into a single step. Code Mode is a pattern for how agents interact with MCP tools more efficiently.

## 2. goose Supports Code Mode

Code Mode support landed in goose v1.17.0 in December 2025. It ships as a platform extension called "Code Mode" that you can enable in the desktop app or CLI.

To enable it:

- **Desktop app:** Click the extensions icon and toggle on "Code Mode"
- **CLI:** Run `goose configure` and enable the Code Mode extension

Since its initial implementation, we've added so many improvements!

## 3. Code Mode Keeps Your Context Window Clean

Every time you install an MCP server (or "extension" in the goose ecosystem), it adds a significant amount of data to your agent's memory. Every tool comes with a tool definition describing what the tool does, the parameters it accepts, and what it returns. This helps the agent understand how to use the tool.

These definitions consume space in your agent's context window. For example, if a single definition takes 500 tokens and an extension has five tools, that is 2,500 tokens gone before you even start. If you use multiple extensions, you could easily double or even decuple that number.

Without Code Mode, your context window could look like this:

```
[System prompt: ~1,000 tokens]
[Tool: developer__shell - 500 tokens]
[Tool: developer__text_editor - 600 tokens]
[Tool: developer__analyze - 400 tokens]
[Tool: slack__send_message - 450 tokens]
[Tool: slack__list_channels - 400 tokens]
[Tool: googledrive__search - 500 tokens]
[Tool: googledrive__download - 450 tokens]
... and so on for every tool in every extension
```

As your session progresses, useful context gets crowded out by tool definitions you aren't even using: the code you are discussing, the problem you are solving, or the instructions you previously gave. This leads to performance degradation and memory loss. While I used to recommend disabling unused MCP servers, Code Mode offers a better fix. It uses three tools that help the agent discover what tools it needs on demand rather than having every tool definition loaded upfront:

1. `search_modules` - Find available extensions
2. `read_module` - Learn what tools an extension offers
3. `execute_code` - Run JavaScript that uses those tools

I wanted to see how true this was so I ran an experiment: I had goose solve a user's bug and put up a PR with and without code mode. Code Mode used 30% fewer tokens for the same task.

| Metric | With Code Mode | Without Code Mode |
|--------|----------------|-------------------|
| Total tokens | 23,339 | 33,648 |
| Input tokens | 23,128 | 33,560 |

## 4. Code Mode Batches Operations Into a Single Tool Call

The token savings do not just come from loading fewer tool definitions upfront. Code Mode also handles the "active" side of the conversation through a method called batching.

When you ask an agent to do something, it typically breaks your request into individual steps, each requiring a separate tool call. You can see these calls appear in your chat as the agent executes the tasks. For example, if you ask goose to "check the current branch, show me the diff, and run the tests," it might run four individual commands:

```
▶ developer__shell → git branch --show-current

▶ developer__shell → git status

▶ developer__shell → git diff

▶ developer__shell → cargo test
```

Each of these calls adds a new layer to the conversation history that goose has to track. Batching combines these into a single execution. When you turn Code Mode on and give that same prompt, you will see just one tool call:

```
▶ Code Execution: Execute Code
  generating...
```

Inside that one execution, it batches all the commands into a script:

```javascript
import { shell } from "developer";

const branch = shell({ command: "git branch --show-current" });
const status = shell({ command: "git status" });
const diff = shell({ command: "git diff" });
const tests = shell({ command: "cargo test" });
```

As a user, you see the same results, but the agent only has to remember one interaction instead of four. By reducing these round trips, Code Mode keeps the conversation history concise so the agent can maintain focus on the task at hand.

## 5. Code Mode Makes Smarter Tool Choices

When an agent has access to dozens of tools, it sometimes makes a "logical" choice that is technically wrong for your environment. This happens because, in a standard setup, the agent picks tools from a flat list based on short text descriptions. This can lead to a massive waste of time and tokens when the agent picks a tool that sounds right but lacks the necessary context.

I saw this firsthand during my experiments. I had an extension enabled called agent-task-queue, which is designed to run background tasks with timeouts.

When I asked goose to run the tests for my PR, it looked at the available tools and saw agent-task-queue. The LLM reasoned that a test suite is a "long-running task," making that extension a perfect fit. It chose the specialized tool over the generic shell.

However, the tool call failed immediately:

```
FAILED exit=127 0.0s
/bin/sh: cargo: command not found
```

My environment was not configured to use that specific extension for my toolchain. goose made a reasonable choice based on the description, but it was the wrong tool for my actual setup.

In the Code Mode session, this mistake never happened. Code Mode changes how the agent interacts with its capabilities by requiring explicit import statements.

Instead of browsing a menu of names, goose had to be intentional about which module it was using. It chose to import from the developer module:

```javascript
import { shell } from "developer";

const test = shell({ command: "cargo test -p goose --lib formats::google" });
```

By explicitly importing developer, Code Mode ensured the tests ran in my actual shell environment.

## 6. Code Mode Is Portable Across Editors

goose is more than an agent; it's also an [ACP (Agent Client Protocol)](/docs/gdk/acp) server. This means you can connect it to any editor that supports ACP, like Zed or Neovim. Plus, any MCP server you use in goose will work there, too.

I wanted to try this myself, so I set up Neovim to connect to goose **with Code Mode enabled**. Here's the configuration I used:

```lua
{
  "yetone/avante.nvim",
  build = "make",
  event = "VeryLazy",
  opts = {
    provider = "goose",
    acp_providers = {
      ["goose"] = {
        command = "goose",
        args = { "acp", "--with-builtin", "code_execution,developer" },
      },
    },
  },
  dependencies = {
    "nvim-lua/plenary.nvim",
    "MunifTanjim/nui.nvim",
  },
}
```

The key line is the one where I enable Code Mode right inside the editor config:

```lua
args = { "acp", "--with-builtin", "code_execution,developer" },
```

To test it, I asked goose to list my Rust files and count the lines of code. Instead of a long stream of individual shell commands cluttering my Neovim buffer, I saw one singular tool call: Code Execution. It worked exactly like it does in the desktop app. This portability means you can build a powerful, efficient agent workflow and take it with you to whatever environment you're most comfortable in.

![Neovim with Code Mode enabled](neovim-code-mode.png)

## 7. Code Mode Performs Differently Across LLMs

I ran my experiments using Claude Opus 4.5. Your results may vary depending on which model you use.

Code Mode requires the LLM to do things that not all models do equally well:

- **Write valid JavaScript** - The model has to generate syntactically correct code. Models with stronger code generation capabilities will produce fewer errors.
- **Follow the import pattern** - Code Mode expects the LLM to import tools from modules like `import { shell } from "developer"`. Some models might try to call tools directly without importing, which will fail.
- **Use the discovery tools** - Before writing code, the LLM should call `search_modules` and `read_module` to learn what tools are available. Some models skip this step and guess, leading to hallucinated tool names.
- **Handle errors gracefully** - When a code execution fails, the model needs to read the error, understand what went wrong, and try again. Some models are better at this feedback loop than others.

If Code Mode is not working well for you, try switching models. A model that excels at code generation and instruction following will generally perform better with Code Mode than one optimized for other tasks.

## 8. Code Mode Is Not for Every Task

Code Mode adds overhead. Before executing anything, the LLM has to:

1. Call `search_modules` to find available extensions
2. Call `read_module` to learn what tools an extension offers
3. Write JavaScript code
4. Call `execute_code` to run it

For simple, single-tool tasks, this overhead is not worth it. If you just need to run one shell command or view one file, regular tool calling is faster.

Based on my experiments, here is when Code Mode makes sense:

| Use Code Mode When | Skip Code Mode When |
|--------------------|---------------------|
| You have multiple extensions enabled | You only have 1-2 extensions |
| Your task involves multi-step orchestration | Your task is a single tool call |
| You want longer sessions without context rot | Speed matters more than context longevity |
| You are working across multiple editors | You are doing a quick one-off task |

## Try It Out

If you want to experiment with Code Mode, here are some resources:

**Documentation:**
- [ACP client setup](/docs/gdk/acp)
- [Extensions guide](/docs/getting-started/using-extensions)

**Previous posts:**
- [Code Mode MCP in goose](/blog/2025/12/15/code-mode-mcp) by Alex Hancock
- [Code Mode Doesn't Replace MCP](/blog/2025/12/21/code-mode-doesnt-replace-mcp) by me

**Community:**
- Join our [Discord](https://discord.gg/n8R5VaWDAn) to share what you learn
- File issues on [GitHub](https://github.com/aaif-goose/goose) if something does not work as expected

Run your own experiments and let us know what you find.

<head>
  <meta property="og:title" content="8 Things You Didn't Know About Code Mode" />
  <meta property="og:type" content="article" />
  <meta property="og:url" content="https://goose-docs.ai/blog/2026/02/06/8-things-you-didnt-know-about-code-mode" />
  <meta property="og:description" content="Discover how Code mode reduces context rot and token usage in AI agents making them more efficient for long running sessions." />
  <meta property="og:image" content="https://goose-docs.ai/assets/images/header-image-bf242a438cd67caab097fab1d8bd31c5.png" />
  <meta name="twitter:card" content="summary_large_image" />
  <meta property="twitter:domain" content="goose-docs.ai" />
  <meta name="twitter:title" content="8 Things You Didn't Know About Code Mode" />
  <meta name="twitter:description" content="Discover how Code mode reduces context rot and token usage in AI agents making them more efficient for long running sessions." />
  <meta name="twitter:image" content="https://goose-docs.ai/assets/images/header-image-bf242a438cd67caab097fab1d8bd31c5.png" />
  <meta name="keywords" content="goose, MCP, Model Context Protocol, Code Mode, AI agents, context rot, token usage, JavaScript, developer tools, ACP, Neovim" />
</head>
