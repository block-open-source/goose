# Custom Distributions of goose

> **Tip:** This is sometimes referred to as "white labelling" — creating a branded or tailored version of an open source project for your organization.

This guide explains how to create custom distributions of goose tailored to your organization's needs—whether that's preconfigured models, custom tools, branded interfaces, or entirely new user experiences.

## Overview

goose's architecture is designed for extensibility. Organizations can create "remixed" versions that:

- **Preconfigure AI providers**: Ship with a specific model (local or cloud) and API credentials
- **Bundle custom tools**: Include proprietary extensions for internal data sources
- **Customize the experience**: Modify branding, UI, and default behaviors
- **Target specific audiences**: Create specialized versions for developers, legal teams, designers, etc.

## Architecture at a Glance

```
┌─────────────────────────────────────────────────────────────────┐
│                        User Interfaces                          │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────────┐  │
│  │  CLI        │  │  Desktop    │  │  Your Custom UI         │  │
│  │  (goose-cli)│  │  (Electron) │  │  (web, mobile, etc.)    │  │
│  └──────┬──────┘  └──────┬──────┘  └────────────┬────────────┘  │
└─────────┼────────────────┼──────────────────────┼───────────────┘
          │                │                      │
          ▼                ▼                      ▼
┌─────────────────────────────────────────────────────────────────┐
│                    goose serve (ACP)                            │
│         ACP HTTP/WebSocket server for custom clients            │
└─────────────────────────────────────────────────────────────────┘
          │
          ▼
┌─────────────────────────────────────────────────────────────────┐
│                      Core (goose crate)                         │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────────┐  │
│  │  Providers  │  │  Extensions │  │  Config & Recipes       │  │
│  │  (AI models)│  │  (MCP tools)│  │  (behavior & defaults)  │  │
│  └─────────────┘  └─────────────┘  └─────────────────────────┘  │
└─────────────────────────────────────────────────────────────────┘
```

## Key Customization Points

| What You Want | Where to Look | Complexity |
|---------------|---------------|------------|
| Preconfigure a model/provider | `config.yaml`, `init-config.yaml`, environment variables | Low |
| Add custom AI providers | `crates/goose/src/providers/declarative/` | Low |
| Bundle custom MCP extensions | `config.yaml` extensions section, `ui/desktop/src/built-in-extensions.json`, `ui/desktop/src/components/settings/extensions/bundled-extensions.json` | Medium |
| Modify system prompts | `crates/goose/src/prompts/` | Low |
| Customize desktop branding | `ui/desktop/` (icons, names, colors) | Medium |
| Build a new UI (web, mobile) | Integrate with `goose serve` over ACP | High |
| Create guided workflows | Recipes (YAML-based task definitions) | Low |
| Build complex multi-step workflows | Recipes with sub-recipes and subagents | Medium |

## Getting Started

### 1. Fork and Clone

```bash
git clone https://github.com/YOUR_ORG/goose.git
cd goose
```

### 2. Choose Your Customization Strategy

- **Configuration-only**: Modify config files and environment variables (no code changes)
- **Extension-based**: Add custom MCP servers for your tools (minimal core changes)
- **Deep customization**: Modify core behavior, UI, or add new providers

### 3. Build and Distribute

See [BUILDING_LINUX.md](BUILDING_LINUX.md) and [ui/desktop/README.md](ui/desktop/README.md) for platform-specific build instructions.

## Important Considerations

### Licensing

goose is licensed under Apache License 2.0 (ASL v2). Custom distributions must:
- Include the original license and copyright notices
- Clearly indicate any modifications made
- Not use "Goose" trademarks in ways that imply official endorsement

For detailed guidance on ASL v2 compliance, see the [Apache License FAQ](https://www.apache.org/foundation/license-faq.html).

### Contributing Back

While you're free to maintain private forks, contributing improvements upstream benefits everyone—including your distribution. Private forks that diverge significantly become expensive to maintain and miss out on security updates and new features. Consider upstreaming generic improvements while keeping only organization-specific customizations private.

### Telemetry

goose includes optional telemetry (via PostHog) to help improve the project. For custom distributions, you can:
- **Disable telemetry**: Set `GOOSE_DISABLE_TELEMETRY=1`
- **Use your own instance**: Modify `crates/goose/src/posthog.rs` to point to your PostHog instance

### Staying Current

To benefit from upstream improvements:
1. Regularly sync your fork with the main repository
2. Keep customizations isolated (config files, separate extension repos) when possible
3. Use recipes for workflow customization rather than code changes
4. Subscribe to release announcements for breaking changes

---

# Appendix: Custom Distribution Scenarios

## A. Preconfigured Local Model Distribution

**Goal**: Ship goose preconfigured to use a local Ollama model, requiring no API keys.

### Steps

1. **Create an init-config.yaml** in your distribution root:

```yaml
# init-config.yaml - Applied on first run if no config exists
GOOSE_PROVIDER: ollama
GOOSE_MODEL: qwen3-coder:latest
```

2. **Set environment defaults** in your launcher script or packaging:

```bash
export GOOSE_PROVIDER=ollama
export GOOSE_MODEL=qwen3-coder:latest
export OLLAMA_HOST=http://localhost:11434  # Or your hosted instance
```

3. **Optionally hide provider selection** in the UI by modifying `ui/desktop/src/` components.

### Technical Details

- Provider configuration: `crates/goose/src/config/base.rs`
- Ollama provider implementation: `crates/goose/src/providers/ollama.rs`
- Config precedence: Environment variables → config.yaml → defaults

---

## B. Corporate Distribution with Managed API Keys

**Goal**: Distribute goose internally with pre-provisioned API keys for a frontier model.

### Steps

1. **Store API keys securely** using goose's secret management:

```yaml
# config.yaml (distributed with your package)
GOOSE_PROVIDER: anthropic
GOOSE_MODEL: claude-sonnet-4-20250514
```

2. **Inject secrets at install time** or via your MDM/configuration management:

```bash
# Secrets are stored in system keyring or ~/.config/goose/secrets.yaml
# if GOOSE_DISABLE_KEYRING=1
goose configure set-secret ANTHROPIC_API_KEY "your-corporate-key"
```

3. **Lock down provider changes** (optional) by modifying the settings UI or using a recipe that enforces the provider.

### Technical Details

- Secret storage: `crates/goose/src/config/base.rs` (SecretStorage enum)
- Keyring integration: Uses system keyring by default, file-based fallback available
- Config file location: `~/.config/goose/config.yaml`

---

## C. Custom Tools for Internal Data Sources

**Goal**: Add MCP extensions that connect to your data lake, internal APIs, or proprietary systems.

### Steps

1. **Create your MCP server** following the [MCP specification](https://modelcontextprotocol.io/):

```python
# Example: internal_data_mcp.py
from mcp.server import Server
from mcp.types import Tool

server = Server("internal-data")

@server.tool()
async def query_data_lake(query: str) -> str:
    """Query the corporate data lake."""
    # Your implementation here
    return results
```

2. **Bundle as a built-in extension** by adding to either:
   - `ui/desktop/src/built-in-extensions.json` (core built-ins surfaced in extension UI)
   - `ui/desktop/src/components/settings/extensions/bundled-extensions.json` (bundled extension catalog in Settings)

Example:

```json
{
  "id": "internal-data",
  "name": "Internal Data Lake",
  "description": "Query corporate data sources",
  "enabled": true,
  "type": "stdio",
  "cmd": "python",
  "args": ["/path/to/internal_data_mcp.py"],
  "env_keys": ["INTERNAL_DATA_API_KEY"],
  "timeout": 300
}
```

3. **Or distribute as a recipe** that enables the extension:

```yaml
# data-analyst.yaml
title: Data Analyst Assistant
description: goose configured for data analysis
instructions: |
  You have access to the corporate data lake. Help users query and analyze data.
extensions:
  - type: stdio
    name: internal-data
    cmd: python
    args: ["/opt/corp-goose/internal_data_mcp.py"]
    description: Corporate data lake access
```

### Technical Details

- Extension types: `crates/goose/src/agents/extension.rs` (ExtensionConfig enum)
- Built-in MCP servers: `crates/goose-mcp/`
- Extension loading: `crates/goose/src/agents/extension_manager.rs`

---

## D. Custom Branding and UI

**Goal**: Rebrand the desktop application with your organization's identity.

### Steps

1. **Replace visual assets** in `ui/desktop/src/images/`:
   - `icon.png`, `icon.ico`, `icon.icns` - Application icons
   - Update splash screens and logos as needed

2. **Modify application metadata** in `ui/desktop/forge.config.ts`:

```typescript
// forge.config.ts
module.exports = {
  packagerConfig: {
    name: 'YourCompany AI Assistant',
    executableName: 'yourcompany-ai',
    icon: 'src/images/your-icon',
    // ...
  },
  // ...
};
```

3. **Update the system prompt** to reflect your branding in `crates/goose/src/prompts/system.md`:

```markdown
You are an AI assistant called [YourName], created by [YourCompany].
...
```

4. **Customize UI components** in `ui/desktop/src/` (React/TypeScript):
   - Color schemes in CSS/Tailwind config
   - Component text and labels
   - Feature visibility

5. **Align packaging and updater names** when rebranding:
   - Update static branding metadata in `ui/desktop/package.json` (`productName`, description) and Linux desktop templates (`ui/desktop/forge.deb.desktop`, `ui/desktop/forge.rpm.desktop`)

   - Set build/release environment variables consistently:
     - `GITHUB_OWNER` and `GITHUB_REPO` for publisher + updater repository lookup
     - `GOOSE_BUNDLE_NAME` for bundle/debug scripts and updater asset naming (defaults to `Goose`)

Example:

```bash
export GITHUB_OWNER="your-org"
export GITHUB_REPO="your-goose-fork"
export GOOSE_BUNDLE_NAME="InsightStream-goose"
```

6. **Use this branding consistency checklist** before release:
   - Application metadata (`forge.config.ts`, `package.json`, `index.html`) uses your distro name
   - Release artifact names and updater lookup names are consistent
   - Desktop launchers (Linux `.desktop` templates) point to the same executable name produced by packaging

### Technical Details

- Electron config: `ui/desktop/forge.config.ts`
- UI entry point: `ui/desktop/src/renderer.tsx`
- System prompts: `crates/goose/src/prompts/`

---

## E. Building a New Interface (Web, Mobile, etc.)

**Goal**: Create an entirely new frontend while leveraging goose's backend.

goose provides two ACP transport options for building custom clients:

### Option 1: ACP HTTP/WebSocket (`goose serve`)

Use `goose serve` for process-separated integrations such as web apps, desktop shells, and other clients:

```bash
# Start the server
GOOSE_SERVER__SECRET_KEY='a-long-random-secret' goose serve

# ACP endpoint available at http://localhost:3284/acp
```

HTTP clients authenticate with the `X-Secret-Key` header. Browser WebSocket clients use the same secret as a `?token=` query parameter because the browser WebSocket API cannot set custom headers:

```text
ws://localhost:3284/acp?token=a-long-random-secret
```

For browser clients served from a non-loopback origin, pass the exact UI origin with `--allowed-origin`. When you pass `--allowed-origin`, it replaces the default loopback origin allowlist, so include every origin the client needs.

For the ACP protocol and client flow, see [Use goose as an ACP agent](documentation/docs/gdk/acp/index.md).

### Option 2: Agent Client Protocol (ACP) over stdio

For richer local integrations (IDEs and embedded agents), run goose as an ACP agent over stdio.

ACP provides:
- **Bidirectional communication**: Agents can request permissions, stream updates, and receive cancellations
- **Rich tool call handling**: Detailed status updates, locations, and content for each tool invocation
- **Session management**: Create, load, and resume sessions with full conversation history
- **MCP server integration**: Dynamically add MCP servers to sessions

**Start goose as an ACP agent**:

```bash
# Run goose as an ACP server on stdio
goose acp --with-builtin developer,memory

# Or programmatically
cargo run -p goose-cli -- acp --with-builtin developer
```

**Key ACP methods**:

| Method | Description |
|--------|-------------|
| `initialize` | Establish connection and exchange capabilities |
| `session/new` | Create a new session with optional MCP servers |
| `session/load` | Resume an existing session by ID |
| `session/prompt` | Send a prompt and receive streaming responses |
| `session/cancel` | Cancel an in-progress prompt |

**Example: Python ACP client** (see `test_acp_client.py` for a complete example):

```python
import subprocess
import json

class AcpClient:
    def __init__(self):
        self.process = subprocess.Popen(
            ['goose', 'acp', '--with-builtin', 'developer'],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            text=True
        )
    
    def send_request(self, method, params=None):
        request = {"jsonrpc": "2.0", "method": method, "id": 1}
        if params:
            request["params"] = params
        self.process.stdin.write(json.dumps(request) + "\n")
        self.process.stdin.flush()
        return json.loads(self.process.stdout.readline())

# Initialize and create session
client = AcpClient()
client.send_request("initialize", {"protocolVersion": "2025-01-01"})
session = client.send_request("session/new", {"cwd": "/path/to/project"})

# Send a prompt (responses stream as notifications)
client.send_request("session/prompt", {
    "sessionId": session["result"]["sessionId"],
    "prompt": [{"type": "text", "text": "List files in this directory"}]
})
```

**ACP notifications** (sent from agent to client):
- `session/notification` with `agentMessageChunk` - Streaming text responses
- `session/notification` with `toolCall` - Tool invocation started
- `session/notification` with `toolCallUpdate` - Tool status/result updates
- `requestPermission` - Agent requests user confirmation for sensitive operations

For the full ACP specification, see the [Agent Client Protocol documentation](https://github.com/anthropics/anthropic-cookbook/tree/main/misc/agent_client_protocol).

### Technical Details

**ACP**:
- ACP server implementation: `crates/goose/src/acp/server.rs`
- CLI integration: `crates/goose-cli/src/cli.rs` (Command::Acp)
- Protocol library: `agent-client-protocol` crate (Rust implementation of ACP)
- Test client example: `test_acp_client.py`

---

## F. Audience-Specific Distributions (Legal, Design, etc.)

**Goal**: Create a version of goose tailored for a specific professional audience.

### Steps

1. **Create a specialized recipe** that defines the experience:

```yaml
# legal-assistant.yaml
title: Legal Research Assistant
description: AI assistant for legal professionals

instructions: |
  You are a legal research assistant. You help lawyers and paralegals with:
  - Case law research
  - Document review and summarization
  - Contract analysis
  - Legal writing assistance
  
  Always cite sources. Flag when you're uncertain. Never provide actual legal advice.

extensions:
  - type: builtin
    name: developer
    description: File and document tools
  - type: stdio
    name: legal-database
    cmd: python
    args: ["/opt/legal-goose/legal_db_mcp.py"]
    description: Legal database search

activities:
  - "Research case law on..."
  - "Summarize this contract..."
  - "Find precedents for..."

settings:
  goose_provider: anthropic
  goose_model: claude-sonnet-4-20250514
```

2. **Customize the UI** to show only relevant features and use domain-appropriate language.

3. **Bundle domain-specific extensions** for specialized data sources (legal databases, design tools, etc.).

### Technical Details

- Recipe format: `crates/goose/src/recipe/mod.rs`
- Recipe loading: `crates/goose/src/recipe/local_recipes.rs`
- Activity suggestions: Shown in UI as quick-start prompts

---

## G. Adding a Custom AI Provider

**Goal**: Add support for a new AI provider or your self-hosted model endpoint.

### Option 1: Declarative Provider (No Code)

Create a JSON file in `~/.config/goose/custom_providers/` or bundle in your distribution:

```json
{
  "name": "my_provider",
  "engine": "openai",
  "display_name": "My Custom Provider",
  "description": "Our internal LLM endpoint",
  "api_key_env": "MY_PROVIDER_API_KEY",
  "base_url": "https://llm.internal.company.com/v1/chat/completions",
  "models": [
    {
      "name": "company-llm-v1",
      "context_limit": 32768
    }
  ],
  "supports_streaming": true,
  "requires_auth": true
}
```

Supported engines: `openai`, `anthropic`, `ollama`

### Option 2: Custom Provider (Code)

For providers with unique APIs, implement the Provider trait:

1. Create a new file in `crates/goose/src/providers/`
2. Implement the `Provider` trait from `base.rs`
3. Register in `crates/goose/src/providers/factory.rs`

### Technical Details

- Declarative providers: `crates/goose/src/config/declarative_providers.rs`
- Provider trait: `crates/goose/src/providers/base.rs`
- Provider registration: `crates/goose/src/providers/factory.rs`
- Example providers: `crates/goose/src/providers/declarative/*.json`

---

## H. Preconfigured Workflows with Recipes

**Goal**: Create standardized, repeatable workflows that users can run with minimal setup.

Recipes are YAML files that define complete goose experiences—instructions, extensions, parameters, and prompts bundled together. They're ideal for custom distributions because they require no code changes and can be distributed as simple files.

### Basic Recipe Structure

```yaml
version: 1.0.0
title: Daily Standup Report Generator
description: Generates standup reports from GitHub activity

# Parameters users can customize at runtime
parameters:
  - key: github_repo
    input_type: string
    requirement: required
    description: "GitHub repository (e.g., 'owner/repo')"
  
  - key: time_period
    input_type: select
    requirement: optional
    default: "24h"
    options: ["24h", "48h", "week"]
    description: "Time period to analyze"

# System instructions for the AI
instructions: |
  You are a standup report generator. Fetch PR and issue data from GitHub,
  analyze activity, and generate a formatted report.
  
  Always save reports to ./standup/standup-{date}.md

# Extensions this recipe needs
extensions:
  - type: builtin
    name: developer
    description: File operations
  - type: stdio
    name: github
    cmd: uvx
    args: ["github-mcp-server"]
    description: GitHub API access

# Quick-start suggestions shown in UI
activities:
  - "Generate today's standup report"
  - "Summarize this week's PRs"

# Initial prompt with parameter substitution
prompt: |
  Generate a standup report for {{ github_repo }} covering the last {{ time_period }}.
```

### Recipe Parameter Types

| Type | Description | Use Case |
|------|-------------|----------|
| `string` | Free-form text input | Names, paths, queries |
| `number` | Numeric input | Counts, limits |
| `boolean` | True/false toggle | Feature flags |
| `date` | Date picker | Time-based filters |
| `file` | File path (content imported) | Document processing |
| `select` | Dropdown from options | Predefined choices |

### Distributing Recipes

1. **Bundle with your distribution** in a known location
2. **Share via URL** - users can import recipes from URLs
3. **Create a recipe library** - a directory of recipes for different use cases

### Technical Details

- Recipe schema: `crates/goose/src/recipe/mod.rs`
- Parameter handling: `crates/goose/src/recipe/template_recipe.rs`
- Recipe validation: `crates/goose/src/recipe/validate_recipe.rs`

---

## I. Complex Workflows with Sub-Recipes and Subagents

**Goal**: Build sophisticated multi-step workflows that orchestrate multiple specialized tasks.

For complex workflows, goose supports two powerful composition mechanisms:

1. **Sub-recipes**: Predefined recipe templates that can be invoked by name
2. **Subagents**: Independent AI agents spawned to handle specific tasks

### Sub-Recipes: Predefined Task Templates

Sub-recipes let you define reusable workflow components that the main recipe can invoke:

```yaml
version: 1.0.0
title: Implementation Planner
description: Creates detailed implementation plans with research

instructions: |
  Create implementation plans through research and iteration.
  Use sub-recipes to delegate specialized research tasks.

# Define available sub-recipes
sub_recipes:
  - name: "find_files"
    path: "./subrecipes/codebase-locator.yaml"
    description: "Locate relevant files in the codebase"
  
  - name: "analyze_code"
    path: "./subrecipes/code-analyzer.yaml"
    description: "Analyze code structure and patterns"
  
  - name: "find_patterns"
    path: "./subrecipes/pattern-finder.yaml"
    # Pre-fill some parameters
    values:
      search_depth: "3"
      include_tests: "true"

extensions:
  - type: builtin
    name: developer

prompt: |
  Create an implementation plan for the requested feature.
  
  Use the available sub-recipes to research the codebase:
  - find_files: Locate relevant source files
  - analyze_code: Understand current implementation  
  - find_patterns: Find similar features to model after
```

Recipes that define `sub_recipes` get the Summon extension automatically. The AI can invoke sub-recipes through Summon's `delegate` tool:

```
delegate(source: "find_files", parameters: {"search_term": "authentication"})
```

### Subagents: Dynamic Task Delegation

Subagents are independent AI instances that run with their own context. They're useful for:

- **Parallel execution**: Multiple tasks running simultaneously
- **Context isolation**: Preventing context window overflow
- **Specialized tasks**: Different model/settings per task

#### Ad-hoc Subagents

Create subagents on-the-fly with custom instructions:

```yaml
prompt: |
  To complete this task:
  
  1. Spawn a subagent to analyze the frontend code:
     delegate(instructions: "Analyze all React components in src/components/ and list their props and state management patterns")
  
  2. Spawn another subagent for the backend:
     delegate(instructions: "Document all API endpoints in src/api/ including their request/response schemas")
  
  3. Synthesize findings from both subagents into a unified report.
```

#### Parallel Subagent Execution

Use `async: true` to run delegates in parallel, then collect each result with `load(source: "<task_id>")`:

```yaml
prompt: |
  Run these analyses in parallel:
  
  delegate(instructions: "Count lines of code by language", async: true)
  delegate(instructions: "Find all TODO comments", async: true)
  delegate(instructions: "List external dependencies", async: true)
  
  Use load(source: "<task_id>") for each returned task id.
  Then combine the results into a codebase health report.
```

#### Subagent Settings Override

Customize model, provider, or behavior per subagent:

```yaml
prompt: |
  Use a faster model for simple tasks:
  
  delegate(
    instructions: "List all files modified in the last week",
    model: "gpt-4o-mini",
    max_turns: 3
  )
  
  Use the full model for complex analysis:
  
  delegate(
    instructions: "Review this code for security vulnerabilities",
    model: "claude-sonnet-4-20250514",
    temperature: 0.1
  )
```

#### Extension Scoping

Limit which extensions a subagent can access:

```yaml
prompt: |
  Create a sandboxed subagent with only file reading capabilities:
  
  delegate(
    instructions: "Analyze the README files in this project",
    extensions: ["developer"]  # Only developer extension, no network access
  )
```

### Example: Multi-Stage Code Review Workflow

```yaml
version: 1.0.0
title: Comprehensive Code Review
description: Multi-stage code review with parallel analysis

sub_recipes:
  - name: "security_scan"
    path: "./subrecipes/security-scanner.yaml"
    sequential_when_repeated: true  # Don't run multiple security scans in parallel
  
  - name: "style_check"
    path: "./subrecipes/style-checker.yaml"
  
  - name: "test_coverage"
    path: "./subrecipes/coverage-analyzer.yaml"

parameters:
  - key: pr_number
    input_type: number
    requirement: required
    description: "Pull request number to review"
  
  - key: review_depth
    input_type: select
    requirement: optional
    default: "standard"
    options: ["quick", "standard", "thorough"]

instructions: |
  Perform a comprehensive code review using specialized sub-recipes.
  
  ## Review Process
  
  ### Phase 1: Parallel Analysis
  Run these checks simultaneously:
  - style_check: Code style and formatting
  - test_coverage: Test coverage analysis
  
  ### Phase 2: Security Review
  After initial checks pass, run security_scan (sequential to avoid conflicts).
  
  ### Phase 3: Synthesis
  Combine all findings into a unified review report with:
  - Critical issues (must fix)
  - Suggestions (should consider)
  - Positive observations (good practices found)

extensions:
  - type: builtin
    name: developer
  - type: stdio
    name: github
    cmd: uvx
    args: ["github-mcp-server"]

prompt: |
  Review PR #{{ pr_number }} with {{ review_depth }} depth.
  
  {% if review_depth == "quick" %}
  Focus only on critical issues and security concerns.
  {% elif review_depth == "thorough" %}
  Perform exhaustive analysis including performance review.
  {% endif %}
  
  Start by fetching the PR details, then orchestrate the review phases.
```

### Best Practices for Complex Workflows

1. **Use sub-recipes for reusable components** - Define once, use across multiple recipes
2. **Parallelize independent tasks** - Multiple subagent calls in one message run concurrently
3. **Use `sequential_when_repeated: true`** - For tasks that shouldn't run in parallel (e.g., database migrations)
4. **Scope extensions appropriately** - Give subagents only the tools they need
5. **Use background delegation for independent work** - Pass `async: true` to `delegate`, then collect results with `load(source: "<task_id>")`
6. **Handle failures gracefully** - Design workflows to continue even if one subagent fails

### Technical Details

- Subagent tool: `crates/goose/src/agents/subagent_tool.rs`
- Subagent execution: `crates/goose/src/agents/subagent_handler.rs`
- Recipe sub_recipes field: `crates/goose/src/recipe/mod.rs` (SubRecipe struct)
- Template rendering: `crates/goose/src/recipe/template_recipe.rs`
