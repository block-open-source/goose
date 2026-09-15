---
title: Set up goose in Zed
sidebar_position: 3
description: See how Zed installs and configures goose as an ACP agent.
---

# Set up goose in Zed

Zed can run goose as an ACP agent. Install goose from the ACP Registry for the
simplest setup, or configure it manually to use your own goose binary and
environment overrides.

## Install goose from the ACP Registry

Zed has built-in ACP Registry support, so it can download and run goose for you
without manual configuration.

1. Open Zed
2. Open Agent Settings
3. Click `Add Agent`, then choose `Install from Registry`
4. Select `goose`

A registry-installed goose runs the same `goose acp` server and reads your
existing goose configuration, so your providers, models, and extensions carry
over. Zed keeps the installed version up to date for you.

## Configure goose manually

Use a custom agent if you want to run your own goose binary, such as a local
development build, or pass environment overrides.

### Prerequisites

Ensure you have both Zed and the goose CLI installed:

- **Zed**: Download from [zed.dev](https://zed.dev/)
- **goose CLI**: Follow the [installation guide](/docs/getting-started/installation)

Verify goose is installed:

```bash
goose --version
```

### Add goose to your Zed settings

1. Open Zed
2. Open Agent Settings, click `Add Agent`, then choose `Add Custom Agent`. Zed
   scaffolds an `agent_servers` entry and opens your settings file
3. Edit the entry so it runs goose:

```json
{
  "agent_servers": {
    "goose": {
      "type": "custom",
      "command": "goose",
      "args": ["acp"]
    }
  }
}
```

You can now interact with goose directly in Zed. ACP sessions use the extensions
enabled in your goose configuration, so their tools are also available in Zed.

## Override the provider and model

By default, goose uses the provider and model defined in your
[configuration file](/docs/guides/config-files). Override them for a specific
agent configuration with the `GOOSE_PROVIDER` and `GOOSE_MODEL` environment
variables.

This example configures two goose agents with different model settings:

```json
{
  "agent_servers": {
    "goose": {
      "type": "custom",
      "command": "goose",
      "args": ["acp"]
    },
    "goose (GPT-4o)": {
      "type": "custom",
      "command": "goose",
      "args": ["acp"],
      "env": {
        "GOOSE_PROVIDER": "openai",
        "GOOSE_MODEL": "gpt-4o"
      }
    }
  }
}
```

## Use Zed MCP servers with goose

MCP servers in Zed's `context_servers` configuration are automatically
available to goose. This lets native Zed features and the goose agent use the
same MCP servers.

```json
{
  "context_servers": {
    "filesystem": {
      "command": "npx",
      "args": [
        "-y",
        "@modelcontextprotocol/server-filesystem",
        "/path/to/allowed/dir"
      ]
    }
  },
  "agent_servers": {
    "goose": {
      "type": "custom",
      "command": "goose",
      "args": ["acp"]
    }
  }
}
```

All MCP servers in `context_servers` are available to goose when they use stdio
(command-based) or HTTP transports. goose does not support servers using the
deprecated SSE transport.

If a server in `context_servers` has the same name as a goose extension, goose
uses its own [configuration](/docs/guides/config-files).
