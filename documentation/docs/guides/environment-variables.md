---
sidebar_position: 11
title: Environment Variables
sidebar_label: Environment Variables
---

goose supports various environment variables that allow you to customize its behavior. This guide provides a comprehensive list of available environment variables grouped by their functionality.

## Model Configuration

These variables control the [language models](/docs/getting-started/providers) and their behavior.

### Basic Provider Configuration

These are the minimum required variables to get started with goose.

| Variable | Purpose | Values | Default |
|----------|---------|---------|---------|
| `GOOSE_PROVIDER` | Specifies the LLM provider to use | [See available providers](/docs/getting-started/providers#available-providers) | None (must be [configured](/docs/getting-started/providers#configure-provider-and-model)) |
| `GOOSE_MODEL` | Specifies which model to use from the provider | Model name (e.g., "gpt-4", "claude-sonnet-4-20250514") | None (must be [configured](/docs/getting-started/providers#configure-provider-and-model)) |
| `GOOSE_TEMPERATURE` | Sets the [temperature](https://medium.com/@kelseyywang/a-comprehensive-guide-to-llm-temperature-%EF%B8%8F-363a40bbc91f) for model responses | Float between 0.0 and 1.0 | Model-specific default |
| `GOOSE_MAX_TOKENS` | Sets the maximum number of tokens for each model response (truncates longer responses) | Positive integer (e.g., 4096, 8192) | Model-specific default |
| `GOOSE_CACHE_TTL` | Sets the Anthropic prompt-cache TTL. `1h` keeps the cached prefix alive across idle gaps (e.g. stepping away mid-session) but bills cache writes at 2x input instead of 1.25x, so it only pays off for sessions that actually idle. Headless runs (`goose run`, subagents, scheduled recipes) always use `5m` | `5m`, `1h` | `5m` |

**Examples**

```bash
# Basic model configuration
export GOOSE_PROVIDER="anthropic"
export GOOSE_MODEL="claude-sonnet-4-5-20250929"
export GOOSE_TEMPERATURE=0.7

# Set a lower limit for shorter interactions
export GOOSE_MAX_TOKENS=4096

# Set a higher limit for tasks requiring longer output (e.g. code generation)
export GOOSE_MAX_TOKENS=16000
```

### Advanced Provider Configuration

These variables are needed when using custom endpoints, enterprise deployments, or specific provider implementations.

| Variable | Purpose | Values | Default |
|----------|---------|---------|---------|
| `GOOSE_PROVIDER__TYPE` | The specific type/implementation of the provider | [See available providers](/docs/getting-started/providers#available-providers) | Derived from GOOSE_PROVIDER |
| `GOOSE_PROVIDER__HOST` | Custom API endpoint for the provider | URL (e.g., "https://api.openai.com") | Provider-specific default |
| `GOOSE_PROVIDER__API_KEY` | Authentication key for the provider | API key string | None |
| `GEMINI3_THINKING_LEVEL` | Sets the [thinking level](/docs/getting-started/providers#gemini-3-thinking-levels) for Gemini 3 models globally | `low`, `high` | `low` |

**Examples**

```bash
# Advanced provider configuration
export GOOSE_PROVIDER__TYPE="anthropic"
export GOOSE_PROVIDER__HOST="https://api.anthropic.com"
export GOOSE_PROVIDER__API_KEY="your-api-key-here"
```

### Claude Thinking Configuration

These variables control Claude's reasoning behavior. Supported on Anthropic and Databricks providers.

| Variable | Purpose | Values | Default |
|----------|---------|---------|---------|
| `CLAUDE_THINKING_TYPE` | Controls Claude reasoning mode | `adaptive`, `enabled`, `disabled` | `adaptive` for Claude 4.6+ models, otherwise `disabled` |

**Examples**

```bash
# Claude 4.6 adaptive thinking
export GOOSE_PROVIDER=anthropic
export GOOSE_MODEL=claude-sonnet-4-6
export CLAUDE_THINKING_TYPE=adaptive

# Explicit extended thinking with the default budget
export CLAUDE_THINKING_TYPE=enabled

# Explicit extended thinking with a larger budget for complex tasks
export CLAUDE_THINKING_TYPE=enabled

# Disable Claude thinking entirely
export CLAUDE_THINKING_TYPE=disabled
```

:::tip Viewing Thinking Output
To see Claude's thinking output in the **CLI**, you also need to set `GOOSE_CLI_SHOW_THINKING=1`. In **goose Desktop**, thinking output is shown automatically in a collapsible "Show reasoning" toggle.
:::

### Planning Mode Configuration

These variables control goose's [planning functionality](/docs/guides/context-engineering/creating-plans).

| Variable | Purpose | Values | Default |
|----------|---------|---------|---------|
| `GOOSE_PLANNER_PROVIDER` | Specifies which provider to use for planning mode | [See available providers](/docs/getting-started/providers#available-providers) | Falls back to GOOSE_PROVIDER |
| `GOOSE_PLANNER_MODEL` | Specifies which model to use for planning mode | Model name (e.g., "gpt-4", "claude-sonnet-4-20250514")| Falls back to GOOSE_MODEL |

**Examples**

```bash
# Planning mode with different model
export GOOSE_PLANNER_PROVIDER="openai"
export GOOSE_PLANNER_MODEL="gpt-4"
```

### Provider Retries

Configurable retry parameters for LLM providers. 

#### AWS Bedrock

| Variable | Purpose | Default |
|---------------------|-------------|---------|
| `BEDROCK_MAX_RETRIES` | The max number of retry attempts before giving up | 6 |
| `BEDROCK_INITIAL_RETRY_INTERVAL_MS` | How long to wait (in milliseconds) before the first retry | 2000 |
| `BEDROCK_BACKOFF_MULTIPLIER` | The factor by which the retry interval increases after each attempt | 2 (doubles every time) |
| `BEDROCK_MAX_RETRY_INTERVAL_MS` | The cap on the retry interval in milliseconds |  120000 |

**Examples**

```bash
export BEDROCK_MAX_RETRIES=10                    # 10 retry attempts
export BEDROCK_INITIAL_RETRY_INTERVAL_MS=1000    # start with 1 second before first retry
export BEDROCK_BACKOFF_MULTIPLIER=3              # each retry waits 3x longer than the previous
export BEDROCK_MAX_RETRY_INTERVAL_MS=300000      # cap the maximum retry delay at 5 min
```

#### Databricks

| Variable | Purpose | Default |
|---------------------|-------------|---------|
| `DATABRICKS_MAX_RETRIES` | The max number of retry attempts before giving up | 3 |
| `DATABRICKS_INITIAL_RETRY_INTERVAL_MS` | How long to wait (in milliseconds) before the first retry | 1000 |
| `DATABRICKS_BACKOFF_MULTIPLIER` | The factor by which the retry interval increases after each attempt | 2 (doubles every time) |
| `DATABRICKS_MAX_RETRY_INTERVAL_MS` | The cap on the retry interval in milliseconds |  30000 |

**Examples**

```bash
export DATABRICKS_MAX_RETRIES=5                      # 5 retry attempts
export DATABRICKS_INITIAL_RETRY_INTERVAL_MS=500      # start with 0.5 second before first retry
export DATABRICKS_BACKOFF_MULTIPLIER=2               # each retry waits 2x longer than the previous
export DATABRICKS_MAX_RETRY_INTERVAL_MS=60000        # cap the maximum retry delay at 1 min
```


## Session Management

These variables control how goose manages conversation sessions and context.

| Variable | Purpose | Values | Default |
|----------|---------|---------|---------|
| `GOOSE_MAX_TURNS` | [Maximum number of turns](/docs/guides/sessions/smart-context-management#maximum-turns) allowed without user input | Integer (e.g., 10, 50, 100) | 1000 |
| `GOOSE_GATEWAY_MAX_TURNS` | Maximum number of turns for gateway sessions (e.g., Telegram). Overrides `GOOSE_MAX_TURNS` for gateway traffic only, so chat platforms can keep a stricter cap than CLI/desktop sessions. | Integer (e.g., 5, 10, 25) | Falls back to `GOOSE_MAX_TURNS`, then 5 |
| `GOOSE_SUBAGENT_MAX_TURNS` | Sets the maximum turns allowed for a [subagent](/docs/guides/context-engineering/subagents) to complete before timeout. Can be overridden by [`settings.max_turns`](/docs/guides/recipes/recipe-reference#settings) in recipes or subagent tool calls. | Integer (e.g., 25) | 25 |
| `GOOSE_MAX_BACKGROUND_TASKS` | Sets the maximum number of concurrent background [subagent](/docs/guides/context-engineering/subagents) tasks goose can run at once | Integer (e.g., 1, 5, 10) | 5 |
| `CONTEXT_FILE_NAMES` | Specifies custom filenames for [hint/context files](/docs/guides/context-engineering/using-goosehints#custom-context-files) | JSON array of strings (e.g., `["CLAUDE.md", ".goosehints"]`) | `[".goosehints", "AGENTS.md"]` |
| `GOOSE_DISABLE_SESSION_NAMING` | Disables automatic AI-generated session naming; avoids the background model call and keeps the default "CLI Session" (goose CLI) or "New Chat" (goose Desktop) | "1", "true" (case-insensitive) to enable | false |
| `GOOSE_PROMPT_EDITOR` | [External editor](/docs/guides/goose-cli-commands#external-editor-mode) to use for composing prompts instead of CLI input | Editor command (e.g., "vim", "code --wait") | Unset (uses CLI input) |
| `GOOSE_CLI_THEME` | [Theme](/docs/guides/goose-cli-commands#themes) for CLI response markdown | "light", "dark", "ansi" | "ansi" |
| `GOOSE_CLI_LIGHT_THEME` | Custom [bat theme](https://github.com/sharkdp/bat#adding-new-themes) for syntax highlighting when using light mode | bat theme name (e.g., "Solarized (light)", "OneHalfLight") | "GitHub" |
| `GOOSE_CLI_DARK_THEME` | Custom [bat theme](https://github.com/sharkdp/bat#adding-new-themes) for syntax highlighting when using dark mode | bat theme name (e.g., "Dracula", "Nord") | "zenburn" |
| `GOOSE_CLI_NEWLINE_KEY` | Customize the keyboard shortcut for [inserting newlines in CLI input](/docs/guides/goose-cli-commands#keyboard-shortcuts) | Single character (e.g., "n", "m") | "j" (Ctrl+J) |
| `GOOSE_CLI_BELL` | Ring the terminal bell when an interactive turn finishes or tool approval is required | "true", "false" | false |
| `GOOSE_CLI_SHOW_THINKING` | Shows model reasoning/thinking output in CLI responses. Some models (e.g., DeepSeek-R1, Kimi, Gemini) expose their internal reasoning process — this variable makes it visible in the CLI. | Set to any value to enable | Disabled |
| `GOOSE_RANDOM_THINKING_MESSAGES` | Controls whether to show amusing random messages during processing | "true", "false" | "true" |
| `GOOSE_CLI_SHOW_COST` | Toggles display of model cost estimates in CLI output | "1", "true" (case-insensitive) to enable | false |
| `GOOSE_MAX_CODE_BLOCK_LINES` | Line count threshold before code blocks are truncated in CLI output. Full content is saved to a temp file. | Positive integer | 50 |
| `GOOSE_TRUNCATED_SHOW_LINES` | Number of lines shown before the "... (N more lines)" message when a code block is truncated | Positive integer | 20 |
| `GOOSE_NO_CODE_TRUNCATION` | Disable code block truncation entirely — all code blocks are shown in full | "1", "true" (case-insensitive) to enable | false |
| `GOOSE_AUTO_COMPACT_THRESHOLD` | Set the percentage threshold at which goose [automatically compacts your session](/docs/guides/sessions/smart-context-management#automatic-compaction). | Float between 0.0 and 1.0 (disabled at 0.0) | 0.8 |
| `GOOSE_TOOL_CALL_CUTOFF` | Number of tool calls to keep in full detail before summarizing older tool outputs to help maintain efficient context usage | Integer (e.g., 5, 10, 20) | Computed from the model context limit and auto-compaction threshold |
| `GOOSE_MOIM_MESSAGE_TEXT` | Injects persistent text into goose's [working memory](/docs/guides/context-engineering/using-persistent-instructions) every turn. Useful for behavioral guardrails or persistent reminders. | Any text string | Not set |
| `GOOSE_MOIM_MESSAGE_FILE` | Path to a file whose contents are injected into goose's [working memory](/docs/guides/context-engineering/using-persistent-instructions) every turn. Supports `~/`. Max 64 KB per file. | File path | Not set |

**Examples**

```bash
# Set a low limit for step-by-step control
export GOOSE_MAX_TURNS=5

# Set a moderate limit for controlled automation
export GOOSE_MAX_TURNS=25

# Set a reasonable limit for production
export GOOSE_MAX_TURNS=100

# Raise the per-gateway cap without changing CLI/desktop limits
# (applies to Telegram and other gateway sessions only)
export GOOSE_GATEWAY_MAX_TURNS=15

# Customize the default subagent turn limit
# Note: This can be overridden per-recipe or per-subagent using the max_turns setting
export GOOSE_SUBAGENT_MAX_TURNS=50

# Use multiple context files
export CONTEXT_FILE_NAMES='["CLAUDE.md", ".goosehints", ".cursorrules", "project_rules.txt"]'

# Disable automatic AI-generated session naming (useful for CI/headless runs)
export GOOSE_DISABLE_SESSION_NAMING=true

# Use vim for composing prompts
export GOOSE_PROMPT_EDITOR=vim

# Set the ANSI theme for the session
export GOOSE_CLI_THEME=ansi

# Customize syntax highlighting themes (uses bat themes)
export GOOSE_CLI_LIGHT_THEME="Solarized (light)"
export GOOSE_CLI_DARK_THEME="Dracula"

# Use Ctrl+N instead of Ctrl+J for newline
export GOOSE_CLI_NEWLINE_KEY=n

# Disable random thinking messages for less distraction
export GOOSE_RANDOM_THINKING_MESSAGES=false

# Show reasoning/thinking output from models that support it (e.g., DeepSeek-R1, Kimi, Gemini)
export GOOSE_CLI_SHOW_THINKING=1

# Enable model cost display in CLI
export GOOSE_CLI_SHOW_COST=true

# Show code blocks up to 100 lines before truncating
export GOOSE_MAX_CODE_BLOCK_LINES=100

# Disable code block truncation entirely (show all lines inline)
export GOOSE_NO_CODE_TRUNCATION=true

# Automatically compact sessions when 60% of available tokens are used
export GOOSE_AUTO_COMPACT_THRESHOLD=0.6

# Keep more tool calls in full detail (useful for debugging or verbose workflows)
export GOOSE_TOOL_CALL_CUTOFF=20

# Inject a persistent reminder into goose's working memory every turn
export GOOSE_MOIM_MESSAGE_TEXT="IMPORTANT: Always run tests before committing changes."

# Load persistent instructions from a file (supports ~/)
export GOOSE_MOIM_MESSAGE_FILE="~/.goose/guardrails.md"
```

### Model Context Limit Overrides

These variables allow you to override the default context window size (token limit) for your models. This is particularly useful when using [LiteLLM proxies](https://docs.litellm.ai/docs/providers/litellm_proxy) or custom models that don't match goose's predefined model patterns.

| Variable | Purpose | Values | Default |
|----------|---------|---------|---------|
| `GOOSE_CONTEXT_LIMIT` | Override context limit for the main model | Integer (number of tokens) | Model-specific default or 128,000 |
| `GOOSE_INPUT_LIMIT` | Override input prompt limit for ollama requests (maps to `num_ctx`) | Integer (number of tokens) | Unset; Ollama uses its model default |

**Examples**

```bash
# Set context limit for main model (useful for LiteLLM proxies)
export GOOSE_CONTEXT_LIMIT=200000
# Override ollama input prompt limit
export GOOSE_INPUT_LIMIT=32000
```

For more details and examples, see [Model Context Limit Overrides](/docs/guides/sessions/smart-context-management#model-context-limit-overrides).

## Tool Configuration

These variables control how goose handles [tool execution](/docs/guides/managing-tools/goose-permissions) and [tool management](/docs/guides/managing-tools/).

| Variable | Purpose | Values | Default |
|----------|---------|---------|---------|
| `GOOSE_MODE` | Controls how goose handles tool execution | "auto", "approve", "chat", "smart_approve" | "auto" |
| `GOOSE_TOOLSHIM` | Enables the [tool shim](/docs/guides/tool-shim) for models that output text-based tool calls | "1", "true" (case-insensitive) to enable | false |
| `GOOSE_TOOLSHIM_BACKEND` | Interpreter backend for the tool shim | "ollama" (default), "local", "llama.cpp" | "ollama" |
| `GOOSE_TOOLSHIM_OLLAMA_MODEL` | Ollama model used as the [tool shim](/docs/guides/tool-shim) interpreter | Model name (e.g. llama3.2, mistral-nemo) | "mistral-nemo" |
| `GOOSE_TOOLSHIM_MODEL` | Model for the local tool shim interpreter backend | Model name | Uses `LOCAL_LLM_MODEL` config |
| `GOOSE_CLI_MIN_PRIORITY` | Controls verbosity of [tool output](/docs/guides/managing-tools/adjust-tool-output) | Float between 0.0 and 1.0 | 0.0 |
| `GOOSE_DEBUG` | Enables debug mode to show full tool parameters without truncation. Can also be toggled during a session using the `/r` [slash command](/docs/guides/goose-cli-commands#slash-commands) | "1", "true" (case-insensitive) to enable | false |
| `GOOSE_SHOW_FULL_OUTPUT` | Shows full tool parameters in CLI output instead of truncating them to the terminal width | true/false | false |
| `GOOSE_SEARCH_PATHS` | Prepends additional directories to PATH for extension commands | JSON array of paths (for example, `["/usr/local/bin", "~/custom/bin"]`) | Built-in search paths followed by the system PATH |
| `GOOSE_MAX_TOOL_RESPONSE_SIZE` | Maximum character count for a single tool response before it is written to a temporary file instead of being included inline in the conversation | Positive integer (e.g., 100000, 200000) | 200000 |
| `GOOSE_SHELL` | Overrides the shell used for Developer extension shell commands | Shell executable path or name (for example, `/bin/zsh`, `pwsh`, `C:\cygwin64\bin\bash.exe`) | Unix: `bash` if found on PATH, otherwise `sh`. Windows: `cmd` |

**Examples**

```bash
# Enable tool interpretation
export GOOSE_TOOLSHIM=true
export GOOSE_TOOLSHIM_OLLAMA_MODEL=llama3.2
export GOOSE_MODE="auto"
export GOOSE_CLI_MIN_PRIORITY=0.2  # Show only medium and high importance output
export GOOSE_SHOW_FULL_OUTPUT=true  # Show full tool parameters in CLI output

# Add custom tool directories for extensions
export GOOSE_SEARCH_PATHS='["/usr/local/bin", "~/custom/tools", "/opt/homebrew/bin"]'

# These custom paths are checked before built-in fallback paths such as
# ~/.local/bin, /usr/local/bin on Unix, Homebrew/MacPorts paths on macOS,
# and finally the inherited system PATH.

# Lower the tool response size limit for smaller-context models
export GOOSE_MAX_TOOL_RESPONSE_SIZE=100000

# Use zsh for Developer extension shell commands
export GOOSE_SHELL=/bin/zsh
```

```bat
REM Windows: use a POSIX-like shell instead of cmd.exe
set GOOSE_SHELL=C:\cygwin64\bin\bash.exe
```

:::note
You only ever set `GOOSE_SHELL` to a shell executable path or name. goose injects the command-line flags automatically based on the shell, so there is no need to add them yourself:

- **PowerShell** (`pwsh`, `powershell`) → `-NoProfile -NonInteractive -Command`
- **cmd** → `/C`
- **POSIX shells** (bash, zsh, … on Windows via Cygwin/MSYS2) → `-c`
- On Unix the default shell (`bash`, falling back to `sh`) is invoked as `<shell> -c`
:::

## Security and Privacy

These variables control security features, credential storage, and anonymous usage data collection.

| Variable | Purpose | Values | Default |
|----------|---------|---------|---------|
| `GOOSE_ALLOWLIST` | Controls which extensions can be loaded | URL for [allowed extensions](/docs/guides/allowlist) list | Unset |
| `GOOSE_DISABLE_KEYRING` | Disables the system keyring for secret storage | Set to any value (e.g., "1", "true", "yes") to disable. The actual value doesn't matter, only whether the variable is set. | Unset (keyring enabled) |
| `SECURITY_PROMPT_ENABLED` | Enable [prompt injection detection](/docs/guides/security/prompt-injection-detection) to identify potentially harmful commands | true/false | false |
| `SECURITY_PROMPT_THRESHOLD` | Sensitivity threshold for prompt injection detection (higher = stricter) | Float between 0.01 and 1.0 | 0.8 |
| `SECURITY_PROMPT_CLASSIFIER_ENABLED` | Enable ML-based prompt injection detection for advanced threat identification | true/false | false |
| `SECURITY_PROMPT_CLASSIFIER_ENDPOINT` | Classification endpoint URL for ML-based prompt injection detection | URL (e.g., "https://api.example.com/classify") | Unset |
| `SECURITY_PROMPT_CLASSIFIER_TOKEN` | Authentication token for `SECURITY_PROMPT_CLASSIFIER_ENDPOINT` | String | Unset |
| `GOOSE_TELEMETRY_ENABLED` | Enable or disable [anonymous usage data collection](/docs/guides/usage-data) | true/false | false |

**Examples**

```bash
# Enable prompt injection detection with default threshold
export SECURITY_PROMPT_ENABLED=true

# Enable with custom threshold (stricter)
export SECURITY_PROMPT_ENABLED=true
export SECURITY_PROMPT_THRESHOLD=0.9

# Enable ML-based detection with external endpoint
export SECURITY_PROMPT_ENABLED=true
export SECURITY_PROMPT_CLASSIFIER_ENABLED=true
export SECURITY_PROMPT_CLASSIFIER_ENDPOINT="https://your-endpoint.com/classify"
export SECURITY_PROMPT_CLASSIFIER_TOKEN="your-auth-token"

# Control anonymous usage data collection
export GOOSE_TELEMETRY_ENABLED=false  # Disable telemetry
export GOOSE_TELEMETRY_ENABLED=true   # Enable telemetry
```

:::tip
When the keyring is disabled (or cannot be accessed and goose [falls back to file-based storage](/docs/troubleshooting/known-issues#keyring-cannot-be-accessed-automatic-fallback)), secrets are stored here:

* macOS/Linux: `~/.config/goose/secrets.yaml`
* Windows: `%APPDATA%\Block\goose\config\secrets.yaml`
:::

## Network Configuration

These variables configure network proxy settings for goose.

### OAuth Callback Port

By default, goose starts a temporary local server on a random port to receive OAuth callbacks. Enterprise identity providers that require exact `redirect_uri` matching (and forbid wildcard ports) will reject the callback. Set this variable to use a fixed port instead.

| Variable | Purpose | Values | Default |
|----------|---------|---------|---------|
| `GOOSE_OAUTH_CALLBACK_PORT` | Fixed port for the local OAuth callback server | Port number (e.g., 8080, 9999) | Random (OS-assigned) |

**Examples**

```bash
# Use a fixed port so your IdP's redirect_uri whitelist can match exactly
export GOOSE_OAUTH_CALLBACK_PORT=8080
```

Then register the appropriate redirect URI in your identity provider:
- For MCP server OAuth: `http://127.0.0.1:8080/oauth_callback`
- For Databricks OAuth: `http://localhost:8080`

### HTTP Proxy

goose supports standard HTTP proxy environment variables for users behind corporate firewalls or proxy servers.

| Variable | Purpose | Values | Default |
|----------|---------|---------|---------|
| `HTTP_PROXY` | Proxy URL for HTTP connections | URL (e.g., `http://proxy.company.com:8080`) | None |
| `HTTPS_PROXY` | Proxy URL for HTTPS connections (takes precedence over `HTTP_PROXY` when both are set) | URL (e.g., `http://proxy.company.com:8080`) | None |
| `NO_PROXY` | Hosts to bypass the proxy | Comma-separated list (e.g., `localhost,127.0.0.1,.internal.com`) | None |

**Examples**

```bash
# Configure proxy for all connections
export HTTPS_PROXY="http://proxy.company.com:8080"
export NO_PROXY="localhost,127.0.0.1,.internal,.local,10.0.0.0/8"

# Or with authentication
export HTTPS_PROXY="http://username:password@proxy.company.com:8080"
export NO_PROXY="localhost,127.0.0.1,.internal"
```

Alternatively, proxy settings can be configured through your operating system's network settings. If you encounter connection issues, see [Corporate Proxy or Firewall Issues](/docs/troubleshooting/known-issues#corporate-proxy-or-firewall-issues) for troubleshooting steps.

## Observability

Beyond goose's built-in [logging system](/docs/guides/logs), you can export telemetry to external observability platforms for advanced monitoring, performance analysis, and production insights.

### Observability Configuration

Configure goose to export telemetry to any [OpenTelemetry](https://opentelemetry.io/docs/) compatible platform.

To enable export, set a collector endpoint:

```bash
export OTEL_EXPORTER_OTLP_ENDPOINT="http://localhost:4318"
```

You can control each signal (traces, metrics, logs) independently with `OTEL_{SIGNAL}_EXPORTER`:

| Variable pattern | Purpose | Values |
|---|---|---|
| `OTEL_EXPORTER_OTLP_ENDPOINT` | Base OTLP endpoint (applies `/v1/traces`, etc.) | URL |
| `OTEL_EXPORTER_OTLP_{SIGNAL}_ENDPOINT` | Override endpoint for a specific signal | URL |
| `OTEL_{SIGNAL}_EXPORTER` | Exporter type per signal | `otlp`, `console`, `none` |
| `OTEL_SDK_DISABLED` | Disable all OTel export | `true` |
| `OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT` | Include model messages and tool arguments/results in exported traces | `true`, `false` (default) |

Additional variables like `OTEL_SERVICE_NAME`, `OTEL_RESOURCE_ATTRIBUTES`,
and `OTEL_EXPORTER_OTLP_TIMEOUT` are also supported.
See the [OTel environment variable spec][otel-env] for the full list.

**Examples:**
```bash
# Export everything to a local collector
export OTEL_EXPORTER_OTLP_ENDPOINT="http://localhost:4318"

# Export only traces, disable metrics and logs
export OTEL_TRACES_EXPORTER="otlp"
export OTEL_METRICS_EXPORTER="none"
export OTEL_LOGS_EXPORTER="none"
export OTEL_EXPORTER_OTLP_ENDPOINT="http://localhost:4318"

# Debug traces to console (no collector needed)
export OTEL_TRACES_EXPORTER="console"

# Sample 10% of traces (reduce volume in production)
export OTEL_TRACES_SAMPLER="parentbased_traceidratio"
export OTEL_TRACES_SAMPLER_ARG="0.1"
```

[otel-env]: https://opentelemetry.io/docs/specs/otel/configuration/sdk-environment-variables/

### Langfuse Integration

These variables configure the [Langfuse integration for observability](/docs/tutorials/langfuse).

| Variable | Purpose | Values | Default |
|----------|---------|---------|---------|
| `LANGFUSE_PUBLIC_KEY` | Public key for Langfuse integration | String | None |
| `LANGFUSE_SECRET_KEY` | Secret key for Langfuse integration | String | None |
| `LANGFUSE_URL` | Custom URL for Langfuse service | URL String | Default Langfuse URL |
| `LANGFUSE_INIT_PROJECT_PUBLIC_KEY` | Alternative public key for Langfuse | String | None |
| `LANGFUSE_INIT_PROJECT_SECRET_KEY` | Alternative secret key for Langfuse | String | None |

## goose ACP Server

These variables configure the `goose serve` ACP server process. They are alternatives to the equivalent `goose serve` flags, and are most often used when [running a remote goose server](/docs/guides/remote-goose-server) and connecting goose Desktop to it.

| Variable | Purpose | Values | Default |
|----------|---------|---------|---------|
| `GOOSE_TLS` | Equivalent to `goose serve --tls`. Recommended for remote servers. | `true`, `false` | `false` |
| `GOOSE_TLS_CERT_PATH` | Equivalent to `goose serve --tls-cert-path`. Must be used with `GOOSE_TLS_KEY_PATH`; setting it enables TLS. | File path | None |
| `GOOSE_TLS_KEY_PATH` | Equivalent to `goose serve --tls-key-path`. Must be used with `GOOSE_TLS_CERT_PATH`; setting it enables TLS. | File path | None |
| `GOOSE_SERVER__SECRET_KEY` | Shared secret required by the ACP endpoint unless `--dangerously-unauthenticated` is used. | Secret string | Required |

**Examples**

```bash
# Start a goose ACP server reachable on the local network over TLS
GOOSE_SERVER__SECRET_KEY='a-long-random-secret' \
goose serve --platform desktop --enable-scheduler --host 0.0.0.0 --port 3000 --tls
```

When TLS is enabled, `goose serve` prints a `GOOSED_CERT_FINGERPRINT=...` line on startup. goose Desktop can use this fingerprint to pin the server certificate. See [Running a Remote goose Server](/docs/guides/remote-goose-server) for the full setup.

## Recipe Configuration

These variables control recipe discovery and management.

| Variable | Purpose | Values | Default |
|----------|---------|---------|---------|
| `GOOSE_RECIPE_PATH` | Additional directories to search for recipes | Colon-separated paths on Unix, semicolon-separated on Windows | None |
| `GOOSE_RECIPE_GITHUB_REPO` | GitHub repository to search for recipes | Format: "owner/repo" (e.g., "aaif-goose/goose-recipes") | None |
| `GOOSE_RECIPE_RETRY_TIMEOUT_SECONDS` | Global timeout for recipe success check commands | Integer (seconds) | Recipe-specific default |
| `GOOSE_RECIPE_ON_FAILURE_TIMEOUT_SECONDS` | Global timeout for recipe on_failure commands | Integer (seconds) | Recipe-specific default |

**Examples**

```bash
# Add custom recipe directories
export GOOSE_RECIPE_PATH="/path/to/my/recipes:/path/to/team/recipes"

# Configure GitHub recipe repository
export GOOSE_RECIPE_GITHUB_REPO="myorg/goose-recipes"

# Set global recipe timeouts
export GOOSE_RECIPE_RETRY_TIMEOUT_SECONDS=300
export GOOSE_RECIPE_ON_FAILURE_TIMEOUT_SECONDS=60
```

## Documentation Configuration

This variable controls where the `goose-doc-guide` skill reads goose documentation from.

| Variable | Purpose | Values | Default |
|----------|---------|---------|---------|
| `GOOSE_DOCS_ROOT` | Documentation root for the `goose-doc-guide` skill, used for [offline/air-gapped docs](/docs/guides/offline-docs) | Local path or HTTP(S) URL containing `goose-docs-map.md` and `docs/` | `https://goose-docs.ai` |

## Development & Testing

These variables are primarily used for development, testing, and debugging goose itself.

| Variable | Purpose | Values | Default |
|----------|---------|---------|---------|
| `GOOSE_PATH_ROOT` | Override the root directory for all goose data, config, and state files | Absolute path to directory | Platform-specific defaults |

**Default locations:**
- macOS: `~/Library/Application Support/Block/goose/`
- Linux: `~/.local/share/goose/`
- Windows: `%APPDATA%\Block\goose\`

When set, goose creates `config/`, `data/`, and `state/` subdirectories under the specified path. Useful for isolating test environments, running multiple configurations, or CI/CD pipelines.

**Examples**

```bash
# Temporary test environment
export GOOSE_PATH_ROOT="/tmp/goose-test"

# Isolated environment for a single command
GOOSE_PATH_ROOT="/tmp/goose-isolated" goose run --recipe my-recipe.yaml

# CI/CD usage
GOOSE_PATH_ROOT="$(mktemp -d)" goose run --recipe integration-test.yaml

# Use with developer tools
GOOSE_PATH_ROOT="/tmp/goose-test" ./scripts/goose-db-helper.sh status
```

## Variables Controlled by goose

These variables are automatically set by goose during command execution.

| Variable | Purpose | Values | Default |
|----------|---------|---------|---------|
| `GOOSE_TERMINAL` | Indicates that a command is being executed by goose, enables [customizing shell behavior](#customizing-shell-behavior) | "1" when set | Unset |
| `AGENT` | Generic agent identifier for cross-tool compatibility, enables tools and scripts to detect when they're being run by goose | "goose" when set | Unset |
| `AGENT_SESSION_ID` | The current session ID for [session-isolated workflows](#using-session-ids-in-workflows), automatically available to STDIO extensions and the Developer extension shell commands | Session ID string (e.g., `20260217_5`) | Unset (only set in extension/shell contexts) |

### Customizing Shell Behavior

Sometimes you want goose to use different commands or have different shell behavior than your normal terminal usage. Common use cases include:
- Skipping expensive shell initialization (e.g. syntax highlighting, custom prompts)
- Blocking interactive commands that would hang the agent (e.g., `git commit`)
- Redirecting to agent-friendly tools (e.g., `rg` instead of `find`)
- Building cross-agent tools and scripts that detect AI agent execution
- Integrating with MCP servers and LLM gateways

This is most useful when using goose CLI, where shell commands are executed directly in your terminal environment.

**How it works:**

goose provides the `GOOSE_TERMINAL` and `AGENT` variables you can use to detect whether goose is the executing agent.

1. When goose runs commands:
   - `GOOSE_TERMINAL` is automatically set to "1"
   - `AGENT` is automatically set to "goose"
2. Your shell configuration can detect this and change behavior while keeping your normal terminal usage unchanged

**Examples:**

```bash
# In ~/.zshenv (for zsh users) or ~/.bashrc (for bash users)

# Block git commit when run by goose
if [[ -n "$GOOSE_TERMINAL" ]]; then
  git() {
    if [[ "$1" == "commit" ]]; then
      echo "❌ BLOCKED: git commit is not allowed when run by goose"
      return 1
    fi
    command git "$@"
  }
fi
```

```bash
# Guide goose toward better tool choices
if [[ -n "$GOOSE_TERMINAL" ]]; then
  alias find="echo 'Use rg instead: rg --files | rg <pattern> for filenames, or rg <pattern> for content search'"
fi
```

```bash
# Detect AI agent execution using standard naming convention
if [[ -n "$AGENT" ]]; then
  echo "Running under AI agent: $AGENT"
  # Apply agent-specific behavior if needed
  if [[ "$AGENT" == "goose" ]]; then
    echo "Detected goose - applying goose-specific settings"
  fi
fi
```

### Using Session IDs in Workflows

STDIO extensions (local extensions that communicate via standard input/output) and the Developer extension's shell commands automatically receive the `AGENT_SESSION_ID` environment variable. This enables you to create session-isolated workflows and make it easier to:
- Coordinate work across multiple tool calls using session-isolated handoff paths
- Isolate worktrees or temporary files by session
- Debug correlation between artifacts and session history

The following example shows how a recipe might use the session ID to hand off information between steps:

```bash
# Create session-specific handoff directory
mkdir -p ~/Desktop/${AGENT_SESSION_ID}/handoff
echo "Results from step 1" > ~/Desktop/${AGENT_SESSION_ID}/handoff/output.txt

# Later steps in the recipe can read from the same location
cat ~/Desktop/${AGENT_SESSION_ID}/handoff/output.txt
```

## Environment Variable Passthrough

The Developer extension's `shell` tool inherits environment variables from your session. This enables workflows that depend on environment configuration, such as authenticated CLI operations and build processes.

See [Environment Variables in Shell Commands](/docs/mcp/developer-mcp#environment-variables-in-shell-commands) for details.

## Enterprise Environments

When deploying goose in enterprise environments, administrators might need to control behavior and infrastructure, or enforce consistent settings across teams. The following environment variables are commonly used:

**Network and Infrastructure** - Control how goose connects to external services and internal infrastructure:
- [Network Configuration](#network-configuration) - Proxy configuration and network settings
- [Advanced Provider Configuration](#advanced-provider-configuration) - Point to internal LLM endpoints (e.g., Databricks, custom deployments)
- [Model Context Limit Overrides](#model-context-limit-overrides) - Configure context limits for LiteLLM proxies and custom models

**Security and Privacy** - Control security and privacy features:
- [Security and Privacy](#security-and-privacy) - Manage security and privacy settings such as extension loading, secrets storage, and usage data collection

**Compliance and Monitoring** - Track usage and export telemetry for auditing:

- [Observability](#observability) - Export telemetry to monitoring platforms (OTLP, Langfuse)

## Notes

- Environment variables take precedence over configuration files.
- For security-sensitive variables (like API keys), consider using the system keyring instead of environment variables.
- Some variables may require restarting goose to take effect.
- When using the planning mode, if planner-specific variables are not set, goose will fall back to the main model configuration.
