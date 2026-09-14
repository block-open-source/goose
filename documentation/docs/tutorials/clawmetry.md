---
description: Observe your goose sessions locally with ClawMetry, with no configuration
---

# Observability with ClawMetry

This tutorial covers how to use ClawMetry to see what your goose sessions did, which models they used, and what they cost. Unlike the tracing integrations, there is nothing to configure: ClawMetry reads the session store goose already writes.

## What is ClawMetry

[ClawMetry](https://clawmetry.com/) is an [open-source](https://github.com/vivekchand/clawmetry) (MIT) observability dashboard for AI agents. It runs on your machine, reads the local files your agent already produces, and serves a dashboard at `http://localhost:8900`. goose is a free runtime in the open source app, and the goose adapter ships in the package.

## Why ClawMetry for goose

- **No instrumentation**: No environment variables, no exporter, no SDK. ClawMetry reads `sessions.db` directly, so past sessions show up too.
- **Local by default**: The dashboard runs on your machine and nothing is sent anywhere. Cloud sync exists but is opt-in and off unless you turn it on.
- **Read-only**: goose owns its session store. ClawMetry always opens it read-only and never writes to it.
- **Real token counts**: goose records usage on disk, so token totals come from your sessions rather than an estimate.
- **Open source**: MIT licensed, and the goose adapter is in the repository you can read.

## Set up ClawMetry

```bash
pip install clawmetry
clawmetry
```

Then open `http://localhost:8900`.

That is the whole setup. There is no goose-side configuration step, because ClawMetry does not sit in the request path.

## Run goose

Use goose exactly as you normally would:

```bash
goose session
```

ClawMetry auto-detects the [session store](/docs/guides/logs#session-records) by resolving goose's data directory the same way goose does:

| Platform | Session store |
| --- | --- |
| macOS and Linux | `$XDG_DATA_HOME/goose/sessions/sessions.db`, defaulting to `~/.local/share/goose/sessions/sessions.db` |
| Windows | `%APPDATA%\Block\goose\data\sessions\sessions.db` |

If [`GOOSE_PATH_ROOT`](/docs/guides/environment-variables) is set, ClawMetry reads `$GOOSE_PATH_ROOT/data/sessions/sessions.db` instead, on every platform. On macOS it also checks `~/Library/Application Support/Block/goose/` last, so an older install that still keeps its data there is picked up.

Sessions you ran before installing ClawMetry appear as well.

## What you see

- **Sessions**: every goose session with its start time, message count, and working directory.
- **Transcripts**: the full turn by turn conversation, including tool calls and their results.
- **Models**: which model each session used, and how usage is split across them.
- **Tokens and cost**: input, output, and total tokens per session, with cost where goose recorded it.

:::note
goose populates a cost figure only for providers that report one. With a local provider such as Ollama there is no cost to record, so ClawMetry shows the token counts and leaves cost empty rather than inventing a number.
:::

:::tip
If you run several agents, the runtime switcher at the top of the dashboard scopes every view to goose alone.
:::

## Learn more

- [ClawMetry repository](https://github.com/vivekchand/clawmetry)
- [The goose adapter source](https://github.com/vivekchand/clawmetry/blob/main/clawmetry/adapters/goose.py)
- [ClawMetry documentation](https://clawmetry.com/docs)
