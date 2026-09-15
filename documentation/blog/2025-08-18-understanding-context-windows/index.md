---
title: "The AI Skeptic’s Guide to Context Windows"
description: Why do AI agents forget? Learn how context windows, tokens, and Goose help you manage memory and long conversations.
authors: 
    - rizel
---

![Context Windows](contextwindow.png)

Working with AI tools can feel like working with a flaky, chaotic, but overconfident coworker. You know, the kind who forgets tasks, lies unprovoked, starts new projects without telling you, then quits halfway through. It's enough to make you say: "Forget it. I'll do it myself." But before we write off AI entirely, it's worth understanding what's actually happening under the hood so we can avoid common pitfalls and make AI tools worth using.

<!--truncate-->

The root of this behavior stems from how AI tools handle working memory. Similarly, as a human, you can only juggle so much information at once. For example, when I'm reading a really long research paper, I might forget key details from the introduction by the time I reach the conclusion, even though those early points were important for understanding the whole argument.

The technical term for AI assistants' working memory is **context window**.

## What is a context window?

A context window is the maximum amount of information an AI model can process in a single session. It's measured in "tokens."

## What are tokens?

Tokens are how AI models break down text for processing. They're roughly equivalent to words or word fragments, though different models vary in how they tokenize text based on their training data and design choices.

**For example:**  
 "Hello" = 1 token  
 "Understanding" = 2 tokens ("Under" + "standing")  
 "AI" = 1 token  
 "Tokenization" = 3 tokens

Test it yourself: paste any text into [OpenAI’s tokenizer tool](https://platform.openai.com/tokenizer) and explore how tokens are counted across models.

### How Goose uses tokens

Let's talk about how this works in practice. When you use an AI agent like Goose, you start a session and choose a model like Claude Sonnet 3.7. This model has a context window of 128,000 tokens. This means every session (or conversation) can handle up to 128,000 tokens. If you message "hey" to Goose, you would have used one token. And when Goose responds back, you would have used several more tokens. Now you've used a small portion of your 128,000 tokens, and you have the remainder left.

:::note
Context windows vary per LLM.
:::

Once the conversation goes past 128,000 tokens or gets close to it, your agent may start to forget key details from earlier in the conversation, and it might prioritize the most recent information.

But your conversation isn't the only thing using your tokens. Here are other things within Goose that consume your token budget:

* **System prompt:** A built-in prompt that instructs your agent on how to behave and defines its identity  
  * The system prompt defines Goose’s name, creator (Block), current date/time, task and extension handling, and response format.  
* **Extensions and their tool definitions** - Many extensions have more than one tool built in. For example, a Google Drive extension may include tools like read file, create file, and comment on file. In addition, each tool comes with instructions on how to use it and an explanation of what the tool does.  
* **Tool response** - The response that the tool returns. For example, the tool could respond with "Here's the entire contents of your 500-line code file."  
* In addition to your conversation history, Goose keeps metadata about your conversation, such as timestamps.

This is a lot of data, and it can easily consume your context window. In addition to impacting performance, token usage affects costs. The more tokens you use, the more money you pay, and you may feel frustrated wasting your tokens on your agent misinterpreting your request.

Luckily, Goose has an intelligent design for helping you save your context window.

## How Goose automatically manages your context window

Goose has a method that auto-compacts (or summarizes) your conversation once it reaches a certain threshold. By default, when you reach 80% of your context window, Goose summarizes the conversation, preserving key parts while compressing the rest, reducing context window usage so you can stay in your session without starting a new one.

You actually have the ability to customize the threshold. If you think 80% is too little or too much for your workflow, you can set the environment variable `GOOSE_AUTO_COMPACT_THRESHOLD` to your preferred threshold.

## How to manage your context window

While Goose is adept at helping you manage your context window, you can proactively manage it, too. Here are some tips for efficiently managing your context window and your wallet.

**1. Manual summarization**

When your conversation gets too long, you can summarize the key points and start a new session. Copy important decisions, code snippets, or project requirements into the fresh session. This way, you keep the essential context without carrying over the full conversation history.

**2. `.goosehints`**

Use [.goosehints](/docs/guides/context-engineering/using-goosehints/) files to avoid repeating the same instructions. Instead of typing out your project context, coding standards, and preferences in every conversation, define them once in a .goosehints file. This prevents wasting tokens on repetitive explanations and helps Goose understand your requirements more quickly.

**3. Memory extension**

The [Memory extension](https://goose-docs.ai/docs/mcp/memory-mcp) stores important information across sessions. Instead of re-explaining your project background, past decisions, or important context every time you start a new conversation, you can reference stored information. This keeps your prompts focused on the current task rather than repeating historical context.

**4. Recipes**

[Recipes](https://goose-docs.ai/docs/guides/recipes/) package complete task setups into reusable configurations, eliminating the need to provide lengthy instructions repeatedly. Instead of consuming tokens explaining complex workflows in every session, recipes contain all necessary instructions, extensions, and parameters upfront. This is particularly valuable for repetitive tasks where you'd otherwise spend significant tokens on setup and explanation. And if your recipe starts to feel overly lengthy, you can break the tasks up into [subrecipes](https://goose-docs.ai/docs/guides/recipes/subrecipes).

**5. Subagents**

[Subagents](https://goose-docs.ai/docs/guides/context-engineering/subagents) handle specific tasks in their own isolated sessions. This prevents your main conversation from getting cluttered with implementation details and tool outputs. You delegate work to subagents and only see the final results, keeping your primary context window clean and focused.

**6. Short sessions**

Keep individual sessions focused on specific tasks. When you complete a task or reach a natural stopping point, start a new session. This prevents context window bloat from accumulated conversation history and ensures your tokens are spent on current, relevant work.

**7. Planner model + focused execution**

Use a strong [reasoning model](/docs/guides/multi-model/) for complex work and keep your default model focused on execution. This gives you control over cost and quality while keeping model behavior explicit and predictable.

---

The next time your AI agent seems to 'forget' something important or goes off track, check your context window usage first. The solution might be a better prompt or a cleaner context window. Often, the difference between flaky and focused is just a few tokens.


<head>
  <meta property="og:title" content="The AI Skeptic’s Guide to Context Windows" />
  <meta property="og:type" content="article" />
  <meta property="og:url" content="https://goose-docs.ai/blog/2025/08/18/understanding-context-windows" />
  <meta property="og:description" content="Why do AI agents forget? Learn how context windows, tokens, and Goose help you manage memory and long conversations." />
  <meta property="og:image" content="https://goose-docs.ai/assets/images/contextwindow-fa46f7a54cfb23a538d62f0e4502e19e.png" />
  <meta name="twitter:card" content="summary_large_image" />
  <meta property="twitter:domain" content="goose-docs.ai" />
  <meta name="twitter:title" content="The AI Skeptic’s Guide to Context Windows" />
  <meta name="twitter:description" content="Why do AI agents forget? Learn how context windows, tokens, and Goose help you manage memory and long conversations." />
  <meta name="twitter:image" content="https://goose-docs.ai/assets/images/contextwindow-fa46f7a54cfb23a538d62f0e4502e19e.png" />
</head>