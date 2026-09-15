---
sidebar_position: 9
title: ACP Providers
sidebar_label: ACP Providers
description: Use ACP agents like Claude Code and Codex as goose providers with extension support
---

# ACP Providers

goose supports [Agent Client Protocol (ACP)](https://agentclientprotocol.com/) agents as providers. ACP is a standard protocol for communicating with coding agents, and there's a growing [registry](https://github.com/agentclientprotocol/registry) of agents that implement it.

ACP providers pass goose [extensions](/docs/getting-started/using-extensions) through to the agent as MCP servers, so the agent can call your extensions directly.

:::tip Use Your Existing Subscriptions
ACP providers let you use goose with your existing Claude Code or ChatGPT Plus/Pro subscriptions — no per-token API costs. They are the recommended replacement for the deprecated [CLI providers](/docs/guides/cli-providers).
:::

:::warning Limitations
- **No session fork or resume**: You can start new sessions, but `goose session resume` and `goose session fork` are not supported yet.
- **ACP session ID differs from goose session ID**: Telemetry fields may not correlate across the two.
:::

## Available ACP Providers

### Amp ACP

Wraps [amp-acp](https://www.npmjs.com/package/amp-acp), an ACP adapter for [Amp](https://ampcode.com). Uses your existing Amp subscription.

**Requirements:**
- Node.js and npm
- Amp CLI installed (`curl -fsSL https://ampcode.com/install.sh | bash`)
- ACP adapter installed (`npm install -g amp-acp`)
- Authenticated with your Amp account (`amp` CLI working)

### Claude ACP

Wraps [claude-agent-acp](https://github.com/agentclientprotocol/claude-agent-acp), an ACP adapter for Anthropic's Claude Code. Uses the same Claude subscription as the deprecated `claude-code` CLI provider.

**Requirements:**
- Node.js and npm
- Active Claude Code subscription
- Authenticated with your Anthropic account (`claude` CLI working)

### Codex ACP

Use goose with ChatGPT Plus/Pro or OpenAI API credits via the [codex-acp](https://github.com/agentclientprotocol/codex-acp) adapter.

**Requirements:**
- Node.js and npm
- Active ChatGPT Plus/Pro subscription or OpenAI API credits
- Authenticated with your OpenAI account (`codex` CLI working)

### Pi ACP

Wraps `pi-acp`, an ACP adapter for Pi. Uses your existing Pi installation.

### Custom ACP agents

Goose can also run a user-defined ACP agent through the **Custom provider** flow.
This is useful for agents that are not built into goose, including Kiro CLI, and it
uses the same provider list after setup. The custom route launches a local process
with a structured executable and argument vector; it never evaluates a shell command.

The built-in Pi provider remains available. A custom Pi configuration is useful as a
smoke test or when you need different launch arguments.

**Requirements:**
- Pi CLI installed
- ACP adapter installed (`pi-acp` binary available)
- Authenticated with your Pi account (`pi` CLI working)

## Setup Instructions

### Amp ACP

1. **Install the Amp CLI**

   ```bash
   curl -fsSL https://ampcode.com/install.sh | bash
   ```

2. **Install the ACP adapter**

   ```bash
   npm install -g amp-acp
   ```

3. **Authenticate with Amp**

   Run `amp` and follow the authentication prompts.

4. **Configure goose**

   Set the provider environment variable:
   ```bash
   export GOOSE_PROVIDER=amp-acp
   ```

   Or configure through the goose CLI using `goose configure`.

### Claude ACP

1. **Install the ACP adapter**

   ```bash
   npm install -g @agentclientprotocol/claude-agent-acp
   ```

2. **Authenticate with Claude**

   Ensure your Claude CLI is authenticated and working

3. **Configure goose**

   Set the provider environment variable:
   ```bash
   export GOOSE_PROVIDER=claude-acp
   ```

   Or configure through the goose CLI using `goose configure`:

   ```bash
   ┌   goose-configure
   │
   ◇  What would you like to configure?
   │  Configure Providers
   │
   ◇  Which model provider should we use?
   │  Claude Code
   │
   ◇  Model fetch complete
   │
   ◇  Enter a model from that provider:
   │  default
   ```

### Codex ACP

1. **Check the installed package**

   ```bash
   codex-acp --version
   ```

   The output should start with `@agentclientprotocol/codex-acp`. If it does, continue to authentication.

2. **Install or replace only if needed**

   If `--version` is rejected, remove `@zed-industries/codex-acp`:

   ```bash
   npm uninstall -g @zed-industries/codex-acp
   ```

   If `codex-acp` is missing or was removed, install `@agentclientprotocol/codex-acp`:

   ```bash
   npm install -g @agentclientprotocol/codex-acp
   ```

3. **Authenticate with OpenAI**

   Run `codex` and follow the authentication prompts. A compatible existing Codex login can be reused.

4. **Configure goose**

   Set the provider and use `current` to let Codex choose its default model:
   ```bash
   export GOOSE_PROVIDER=codex-acp
   export GOOSE_MODEL=current
   ```

   Or configure through the goose CLI using `goose configure`:

   ```bash
   ┌   goose-configure
   │
   ◇  What would you like to configure?
   │  Configure Providers
   │
   ◇  Which model provider should we use?
   │  Codex CLI
   │
   ◇  Model fetch complete
   │
   ◇  Enter a model from that provider:
   │  current
   ```

Replacing the npm package does not change `~/.codex` or require recreating your goose configuration. goose does not replace the package automatically.

### Custom ACP agent (Kiro example)

1. Install and authenticate the agent. For Kiro, verify that `kiro-cli acp` works.
2. Run `goose configure` and select **Custom Providers**, then **Add A Custom Provider**.
3. Select **ACP agent (local stdio)**.
4. Enter a display name such as `Kiro ACP`, command `kiro-cli`, and arguments
   `acp` (or `acp, --agent, my-agent` for a named Kiro agent).
5. Run `goose configure` again and select the saved provider from the normal provider
   list.

The equivalent persisted shape is:

```json
{
  "name": "custom_kiro_acp",
  "engine": "acp",
  "display_name": "Kiro ACP",
  "base_url": "",
  "models": [],
  "requires_auth": false,
  "acp": {
    "command": "kiro-cli",
    "args": ["acp"],
    "env": [],
    "env_remove": [],
    "work_dir": null,
    "model_config_option_id": "model",
    "session_config_options": []
  }
}
```

The ACP agent owns authentication and model availability. Goose forwards configured
MCP extensions and uses the existing ACP lifecycle, but unsupported capabilities
remain agent-dependent.

The custom command is launched with goose's working directory and receives the
configured `work_dir` as the session working directory, exactly like the built-in
ACP providers. Set `work_dir` when the agent should treat a specific directory as
the session root; the agent still decides which files and tools it may use.

`env` adds variables for the agent process and `env_remove` strips inherited ones;
values in `env` are stored in the provider file, so keep secrets out of it and let
the agent read them from your environment instead.

An optional `mode_mapping` maps goose's permission modes to the mode IDs your agent
offers, for example `{"auto": ["agent-auto"], "approve": ["agent-ask"]}`. This
changes which actions the agent may take without asking, so set it deliberately and
only with mode IDs the installed agent actually reports. If a mapped mode is not
offered, goose refuses to start the provider rather than leaving the agent in its
own default mode. `mode_mapping` is only available by editing the provider file.

### Custom ACP agent (Pi example)

Use command `pi-acp` with no arguments and save it as `Pi custom ACP`. This exercises
the generic path and does not replace the built-in `pi-acp` provider.

### Pi ACP

1. **Install the Pi CLI and ACP adapter**

   Install the `pi` CLI and the `pi-acp` ACP adapter following the project's installation instructions.

2. **Authenticate with Pi**

   Run `pi` and follow the authentication prompts.

3. **Configure goose**

   Set the provider environment variable:
   ```bash
   export GOOSE_PROVIDER=pi-acp
   ```

   Or configure through the goose CLI using `goose configure`.

## Usage Examples

### Basic Usage

```bash
goose session
```

### Using with Extensions

Extensions configured via `--with-extension` or `--with-streamable-http-extension` are passed through to the ACP agent:

```bash
GOOSE_PROVIDER=claude-acp goose run \
  --with-extension 'npx -y @modelcontextprotocol/server-everything' \
  -t 'Use the echo tool to say hello'
```

```bash
GOOSE_PROVIDER=codex-acp goose run \
  --with-streamable-http-extension 'https://mcp.kiwi.com' \
  -t 'Search for flights from BKI to SYD tomorrow'
```

## Configuration Options

### Amp ACP Configuration

| Environment Variable | Description       | Default   |
|----------------------|-------------------|-----------|
| `GOOSE_PROVIDER`     | Set to `amp-acp`  | None      |
| `GOOSE_MODEL`        | Model to use      | `current` |
| `GOOSE_MODE`         | Permission mode   | `auto`    |

### Claude ACP Configuration

| Environment Variable | Description         | Default   |
|----------------------|---------------------|-----------|
| `GOOSE_PROVIDER`     | Set to `claude-acp` | None      |
| `GOOSE_MODEL`        | Model to use        | `default` |
| `GOOSE_MODE`         | Permission mode     | `auto`    |

**Known Models:**
- `default` (opus)
- `sonnet`
- `haiku`

**Permission Modes (`GOOSE_MODE`):**

| Mode            | Session Mode        | Behavior                                              |
|-----------------|---------------------|-------------------------------------------------------|
| `auto`          | `bypassPermissions` | Skips all permission checks                           |
| `smart-approve` | `acceptEdits`       | Auto-accepts file edits, prompts for risky operations |
| `approve`       | `default`           | Prompts for all permission-required operations        |
| `chat`          | `plan`              | Planning only, no tool execution                      |

See [claude-agent-acp](https://github.com/agentclientprotocol/claude-agent-acp) for session mode details.

### Codex ACP Configuration

| Environment Variable | Description        | Default   |
|----------------------|--------------------|-----------|
| `GOOSE_PROVIDER`     | Set to `codex-acp` | None      |
| `GOOSE_MODEL`        | Model to use       | `current` |
| `GOOSE_MODE`         | Permission mode    | `auto`    |

Codex ACP reports its available models dynamically. Keep `current` to use Codex's default, or select a discovered model explicitly.

**Permission Modes (`GOOSE_MODE`):**

| goose mode      | Codex ACP mode      |
|-----------------|---------------------|
| `auto`          | `agent-full-access` |
| `smart-approve` | `agent`             |
| `approve`       | `read-only`         |
| `chat`          | `read-only`         |

See [codex-acp](https://github.com/agentclientprotocol/codex-acp) for session mode details.

### Pi ACP Configuration

| Environment Variable | Description      | Default   |
|----------------------|------------------|-----------|
| `GOOSE_PROVIDER`     | Set to `pi-acp`  | None      |
| `GOOSE_MODEL`        | Model to use     | `current` |
| `GOOSE_MODE`         | Permission mode  | `auto`    |

## Error Handling

ACP providers depend on external binaries, so ensure:

- The ACP agent binary is installed and in your PATH (`amp-acp`, `claude-agent-acp`, `codex-acp`, `pi-acp`, `copilot`, or your configured custom command)
- The underlying CLI tool is authenticated and working
- Subscription limits are not exceeded
- Node.js and npm are installed (for npm-distributed adapters)

If goose can't find the binary, session startup will fail with an error. Run `which <binary>` to verify installation.
