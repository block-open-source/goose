---
title: "Intro to Agent Client Protocol (ACP): The Standard for AI Agent-Editor Integration"
description: Fix the awkward gap between AI agents and code editors with the Agent Client Protocol (ACP). Learn why this new open standard makes agents like goose truly editor-agnostic, improving AI-human collaboration and restoring developer flow state. ACP works alongside protocols like MCP to create an open AI tooling ecosystem.
authors: 
    - rizel
---

![Choose Your IDE](choose-your-ide.png)

I code best when I sit criss-cross applesauce on my bed or couch with my laptop in my lap, a snack nearby, and no extra screens competing for my attention. Sometimes I keep the editor and browser side by side; other times, I make them full screen and switch between applications. I don't like using multiple monitors, and my developer environment is embarrassingly barebones. 

The described setup allows me to fall into a deep flow state, which is essential for staying productive as a software engineer. It gives me the focus to dig beneath the surface of a problem, trace its root cause, and think about how every fix or improvement affects both users and the system as a whole. While quick bursts of multitasking may work well for other fields, real productivity in engineering often comes from long stretches of uninterrupted thought.

Recently, my workflow changed.

<!--truncate-->

## The Three-App Problem

Now that I use an AI agent regularly, I need to juggle three applications: my editor, my AI agent, and my browser to see what gets rendered. This creates several challenges that disrupt my flow:

- **Reduced screen real estate.** When I try to keep all three windows visible at once in a split-screen layout, each window becomes cramped. My editor shrinks to a narrow column, making it hard to read code. The AI agent window can't display full responses without scrolling. And my browser preview gets squeezed into a tiny viewport that doesn't reflect how users will actually see the app.

- **The temptation to go agent-only.** To avoid the cramped feeling, I sometimes keep only the AI agent window open full-screen, trusting whatever it generates without switching to my editor to review the actual code changes or examine diffs. While this works fine for experimental projects, production systems require human oversight and careful code review.

- **Awkward context switching.** My other approach is to keep the AI agent full-screen but actually do the responsible thing and review its work. I'll read the agent's suggestion, switch windows to my editor to see the change, switch to my browser to check what rendered, then switch back to the agent to continue refining the code through iterative prompts (what Patrick Erichsen from [Continue](https://continue.dev) calls [chiseling](https://patrickerichsen.com/chiseling)). This constant window-switching breaks my concentration and creates opportunities for distraction (hello, Twitter/X).

## Common Workarounds

Many developers have tried to solve this integration challenge:

- **IDE-integrated agents** like Cursor have an AI agent baked into the code editor. However, this creates vendor lock-in where you must use their specific agent with their specific editor. If I preferred VS Code as an editor and Claude Code as my agent, I'd be out of luck. I can't mix and match the tools I want.

- **CLI agents in the terminal** work for some people. They run the agent directly in their editor's terminal pane. [goose](/) has a CLI I could use this way, but I prefer the desktop app's interface for readability and navigating responses. The tradeoff is the constant window-switching I was trying to avoid.

- **IDE extensions and plugins** seem like an obvious solution, but maintaining these integrations is incredibly difficult. In the past year, multiple maintainers built VS Code extensions and even an IntelliJ plugin for goose, but our twice-weekly releases quickly made them outdated. Maintaining these extensions became a constant game of catch-up that our community couldn't win. Extensions have to mirror goose's functionality to work properly, so every change to goose requires updating the extensions. Maintainers couldn't keep pace, and building specific integrations for every editor simply doesn't scale when agents evolve at such a fast pace.

## Introducing Agent Client Protocol

Zed Industries, the creators of the Zed code editor, developed a solution called [Agent Client Protocol (ACP)](https://agentclientprotocol.com/overview/introduction) that resolves these integration challenges. ACP allows you to bring any AI agent into any supporting editor without vendor lock-in. More importantly, it solves the maintenance problem because the AI agent and editor communicate directly through a standardized protocol.

This standardization is achieved by defining a common language via JSON-RPC. Instead of every editor and agent building private, complex handshakes, ACP uses a simple, predictable sequence of structured messages to manage the agent-editor session:

- **session/initialize:** Your AI agent tells the editor what capabilities it supports (audio, text prompts, etc.)
- **session/new:** When you start a new session, the agent and editor establish communication
- **session/prompt:** When you send a prompt, the agent receives and processes it
- **session/update:** The agent sends responses back to the editor
- **session/cancel:** When you cancel a session, the agent stops processing

Today, editors that support ACP include Zed, Neovim, and Marimo. Supported agents include Claude Code, Codex CLI, Gemini, StackPack, and of course, goose.

## Restoring Developer Flow

For developers like me who have specific ways of achieving flow state, this means we can add AI assistance without completely restructuring our work environment. I can keep my criss-cross applesauce position, my split-screen setup, and my focused workflow while having goose integrated directly into my editor.

Beyond personal workflow preferences, ACP lowers the barrier to innovation by allowing AI agents and editors to evolve independently while speaking a shared language. And it's part of a broader movement toward open standards in AI tooling.

You might have heard of [MCP](http://modelcontextprotocol.io), which standardizes how AI agents connect to data sources and tools. ACP and MCP complement each other perfectly: MCP handles the *what* (what data and tools can agents access), while ACP handles the *where* (where the agent lives in your workflow). Together, they create an ecosystem where developers can mix and match the best tools without vendor lock-in.

The goose team continues working to keep goose cutting-edge in the AI agent space, and we're excited about a future where open protocols let developers work however they work best.

## See ACP in Action

If you're ready to see how fast and simple this setup really is, watch the full livestream recording of my ACP setup with goose below

<iframe class="aspect-ratio" src="https://www.youtube.com/embed/Hvu5KDTb6JE" title="Vibe Code with goose: Intro to ACP" frameborder="0" allow="accelerometer; autoplay; clipboard-write; encrypted-media; gyroscope; picture-in-picture" allowfullscreen></iframe>

*Ready to integrate goose directly into your editor? Get started with our [ACP setup guide](https://goose-docs.ai/docs/gdk/acp) and share your experience in our [Discord community](http://discord.gg/n8R5VaWDAn).*


<head>
  <meta property="og:title" content="Intro to Agent Client Protocol (ACP): The Standard for AI Agent-Editor Integration" />
  <meta property="og:type" content="article" />
  <meta property="og:url" content="https://goose-docs.ai/blog/2025/10/24/intro-to-agent-client-protocol-acp" />
  <meta property="og:description" content="Fix the awkward gap between AI agents and code editors with the Agent Client Protocol (ACP). Learn why this new open standard makes agents like goose truly editor-agnostic, improving AI-human collaboration and restoring developer flow state. ACP works alongside protocols like MCP to create an open AI tooling ecosystem." />
  <meta property="og:image" content="https://goose-docs.ai/assets/images/choose-your-ide-c308664c1783e1651d9a4f4d6ff7d731.png" />
  <meta name="twitter:card" content="summary_large_image" />
  <meta property="twitter:domain" content="goose-docs.ai" />
  <meta name="twitter:title" content="Intro to Agent Client Protocol (ACP): The Standard for AI Agent-Editor Integration" />
  <meta name="twitter:description" content="Fix the awkward gap between AI agents and code editors with the Agent Client Protocol (ACP). Learn why this new open standard makes agents like goose truly editor-agnostic, improving AI-human collaboration and restoring developer flow state. ACP works alongside protocols like MCP to create an open AI tooling ecosystem." />
  <meta name="twitter:image" content="https://goose-docs.ai/assets/images/choose-your-ide-c308664c1783e1651d9a4f4d6ff7d731.png"/>
</head>