---
title: "Does Your AI Agent Need a Plan?"
description: Planning with an AI produces good results. Knowing when and how to plan with an AI agent produces even better ones.
authors:
    - rizel
---

![blog cover](blog-banner.png)

:::warning Outdated
The CLI `/plan` command described in this post has since been removed from goose, along with the `GOOSE_PLANNER_PROVIDER` and `GOOSE_PLANNER_MODEL` settings. The other approaches described here still work.
:::

# Does Your AI Agent Need a Plan?

To plan or not to plan, that's the wrong question. Rather than a binary yes/no, planning exists on a spectrum. The real question is which approach fits your current task and working style.

Different developers approach planning in different ways. One builder might draft detailed pseudocode before touching a keyboard, while another practices test driven development to let the architecture emerge organically. You'll find teams sketching complex diagrams on whiteboards and others spinning up fast prototypes to "fail fast" and refactor later.

If planning is a spectrum when coding manually, why wouldn't it be a spectrum when using an agent to code as well?

<!-- truncate -->

Lately, there's been a healthy debate in the industry about planning in AI coding agents. While some find dedicated plan modes essential, others see them as unnecessary overhead. After all, you can always just tell an agent to "make a plan first." Some even argue that if you need a durable plan, you should write it in a file yourself so you can see it, edit it, and version it alongside your code.

This reveals an interesting truth: the value of a plan mode isn't just about the plan itself. It's about creating the right mental model and workflow for the developer using it. Sometimes you want the agent to just execute. Other times, you want to see its thinking, provide feedback, and collaborate on the approach before any code changes happen.

Rather than picking one philosophy, [goose](https://github.com/aaif-goose/goose) supports multiple approaches because different situations call for different methods.

---

## Choose Your Strategy

### For The Architect
**`/plan` Mode**

When you enter plan mode in the goose CLI, goose shifts into an interactive dialogue. Instead of immediately executing, it asks clarifying questions to understand your project deeply. It might ask about your tech stack preferences, authentication requirements, deployment targets, or how you want to handle error cases. This back and forth continues until goose has enough context to generate a comprehensive, actionable plan.

Plan mode uses a separate planner configuration that you can customize. By setting **`GOOSE_PLANNER_PROVIDER`** and **`GOOSE_PLANNER_MODEL`** [environment variables](/docs/guides/environment-variables), you can use one model for strategic planning and a different model for execution. When you're satisfied with the plan, goose asks if you want to clear the message history and act on it, giving you a clear checkpoint before any code changes happen.

I used this approach recently when converting a static Vite/React project to Next.js. I understood the scope clearly since it's a common migration pattern, so I asked goose to make a comprehensive plan before starting any work. It produced an 11 phase migration plan with specific checkboxes for each step, covering everything from dependency updates to routing changes to component boundaries. Once I approved, I said "yes start" and goose executed methodically, committing after each phase.

### For The Director
**Instruction Files**

Sometimes you already know exactly what needs to happen. You've thought through the steps, you've made the decisions, and you just need goose to do the work. Instead of explaining your plan through conversation, you write it down and hand it over.

You can write your instructions in a markdown file as a detailed execution plan, a living document that guides goose through implementation step by step. The plan can include context about the codebase, specific files to modify, expected outcomes, and validation steps. When you're ready, you [run it](/docs/guides/running-tasks) with `goose run -i plan.md` and goose executes what you've specified.

This approach works when you've already done the thinking. Maybe you sketched the architecture on a whiteboard. Maybe you wrote a technical design doc. Maybe you just know this codebase well enough that you don't need goose to ask clarifying questions. You write the spec, goose executes it.

You can also run instruction files in [headless mode](/docs/tutorials/headless-goose) for CI/CD pipelines or automation, but that's just one use case. The core idea is: you own the plan, goose owns the execution.

[Learn more about running tasks →](/docs/guides/running-tasks)


### For The Explorer
**Conversational Context Building**

This approach combines three goose features that work together:

**Conversational planning** means treating goose as a pairing partner rather than a task executor. You ask goose to analyze, explain, and explore. You build a shared mental model together. Then, when you're ready, you shift into execution.

**The [todo extension](/docs/mcp/todo-mcp)** watches for complexity in the background. When goose recognizes that a task has two or more steps, involves multiple files, or has uncertain scope, it automatically creates a checklist. As goose works, it updates progress and checks off completed items. The plan emerges from the work rather than preceding it.

**Project rules** provide invisible scaffolding. Using files like **[`goosehints`](/docs/guides/context-engineering/using-goosehints)** or **`agents.md`**, you encode persistent preferences, commit policies, testing requirements, project conventions, that automatically steer the agent in the right direction. This gives goose the context to make better decisions without you repeating the rules every time.

Together, these features let you have a casual, exploratory conversation while maintaining structure underneath. You scope your prompts deliberately. The todo extension creates organization when complexity appears. The project rules ensure your preferences are always in play.

This is typically how I work. When I migrated a legacy LLM credit provisioning app to Next.js, many cringed at my approach. However, in context, I was returning to a codebase I'd built eight months earlier and didn't remember well. The app was split across two repositories and I didn't know which one handled what. Writing a plan.md file upfront would have been guessing.

So I asked goose to analyze both projects and explain how they communicated. I scoped my prompts deliberately: "just the frontend, no API calls." I had the todo extension enabled, knowing it would create structure once the scope became clear. I had project rules configured to handle commits automatically.

The approach took more back and forth than an upfront plan would have. But those prompts weren't wasted effort. They were building the context that made the actual migration possible. By the time goose created its checklist, we both understood what needed to happen.

[Learn more about the todo extension →](/docs/mcp/todo-mcp)  
[Configure your project rules with goosehints →](/docs/guides/context-engineering/using-goosehints)

---

## What's Your Style?

goose supports multiple planning philosophies because developers don't work in a single mode. The architect wants clarity before code. The director wants control. The explorer discovers the plan through the work.

None of these approaches is superior. Each fits different situations. The same developer might use `/plan` mode for a well scoped migration on Monday and conversational context building for an unfamiliar codebase on Tuesday.

The question isn't whether to plan. The question is which kind of planning fits your situation today.

---

*Ready to try different planning approaches with goose? Start with our [quickstart guide](/docs/quickstart) or explore the [context engineering documentation](/docs/guides/context-engineering) to set up your scaffolding.*

<head>
  <meta property="og:title" content="Does Your AI Agent Need a Plan?" />
  <meta property="og:type" content="article" />
  <meta property="og:url" content="https://goose-docs.ai/blog/2025/12/19/does-your-ai-agent-need-a-plan" />
  <meta property="og:description" content="Planning with an AI produces good results. Knowing when and how to plan with an AI agent produces even better ones." />
  <meta property="og:image" content="https://goose-docs.ai/assets/images/blog-banner-69252aa3455f8a3a303f102c530922f3.png" />
  <meta name="twitter:card" content="summary_large_image" />
  <meta property="twitter:domain" content="goose-docs.ai" />
  <meta name="twitter:title" content="Does Your AI Agent Need a Plan?" />
  <meta name="twitter:description" content="Planning with an AI produces good results. Knowing when and how to plan with an AI agent produces even better ones." />
  <meta name="twitter:image" content="https://goose-docs.ai/assets/images/blog-banner-69252aa3455f8a3a303f102c530922f3.png" />
  <meta name="keywords" content="goose, AI planning, AI agents, plan mode, developer workflow, context engineering, goosehints, todo extension, AI coding assistant, software development" />
</head>
