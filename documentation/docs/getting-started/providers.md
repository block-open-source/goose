---
sidebar_position: 2
title: Configure LLM Provider
---

import Tabs from '@theme/Tabs';
import TabItem from '@theme/TabItem';
import { PanelLeft } from 'lucide-react';
import { ModelSelectionTip } from '@site/src/components/ModelSelectionTip';
import { OnboardingProviderSetup } from '@site/src/components/OnboardingProviderSetup';

# Supported LLM Providers

goose is compatible with a wide range of LLM providers, allowing you to choose and integrate your preferred model.

:::tip Model Selection
<ModelSelectionTip/>
[Berkeley Function-Calling Leaderboard][function-calling-leaderboard] can be a good guide for selecting models.
:::

## Available Providers

| Provider                                                                    | Description                                                                                                                                                                                                               | Parameters                                                                                                                                                                          |
|-----------------------------------------------------------------------------|---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|-------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| [AI/ML API](https://aimlapi.com/)                                           | One API key for 300+ chat, image, video, audio, and embedding models from many providers, OpenAI-compatible. | `AIMLAPI_API_KEY` |
| [Amazon Bedrock](https://aws.amazon.com/bedrock/)                           | Offers a variety of foundation models, including Claude, Jurassic-2, and others. **AWS environment variables must be set in advance, not configured through `goose configure`**                                           | Credential auth: `AWS_PROFILE`, or `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_REGION`<br /><br />Bearer token auth: `AWS_BEARER_TOKEN_BEDROCK` and `AWS_REGION`, `AWS_DEFAULT_REGION`, or `AWS_PROFILE` |
| [Amazon SageMaker TGI](https://docs.aws.amazon.com/sagemaker/latest/dg/realtime-endpoints.html) | Run Text Generation Inference models through Amazon SageMaker endpoints. **AWS credentials must be configured in advance.** | `SAGEMAKER_ENDPOINT_NAME`, `AWS_REGION` (optional), `AWS_PROFILE` (optional)  |
| [Anthropic](https://www.anthropic.com/)                                     | Offers Claude, an advanced AI model for natural language tasks.                                                                                                                                                           | `ANTHROPIC_API_KEY`, `ANTHROPIC_HOST` (optional)                                                                                                                                                                 |
| [Atomic Chat](https://github.com/AtomicBot-ai/Atomic-Chat)                | Run local models with Atomic Chat's OpenAI-compatible server. **Because this provider runs locally, you must first [download a model](#local-llms).** | None required. Connects to local server at `localhost:1337` by default. |
| [Avian](https://avian.io/)                                                   | Cost-effective inference API with DeepSeek, Kimi, GLM, and MiniMax models. OpenAI-compatible with streaming and function calling support.                                                                                  | `AVIAN_API_KEY`, `AVIAN_HOST` (optional)                                                                                                                                            |
| [Azure AI Foundry](/docs/guides/azure-foundry-provider) | Access OpenAI, Anthropic, Microsoft, Meta, Mistral, DeepSeek, GLM, Kimi, and other models deployed through Azure AI Foundry project or MaaS endpoints. | `AZURE_FOUNDRY_ENDPOINT`, `AZURE_FOUNDRY_API_KEY` (optional), `AZURE_FOUNDRY_AD_TOKEN` (optional), `AZURE_FOUNDRY_API_VERSION` (optional) |
| [Azure OpenAI](https://learn.microsoft.com/en-us/azure/ai-services/openai/) | Access Azure-hosted OpenAI models, including GPT-4 and GPT-3.5. Supports API key, Entra ID bearer token, and Azure credential chain authentication.                                                                                          | `AZURE_OPENAI_ENDPOINT`, `AZURE_OPENAI_DEPLOYMENT_NAME`, `AZURE_OPENAI_API_KEY` (optional), `AZURE_OPENAI_AD_TOKEN` (optional)                                                                                           |
| [ChatGPT Codex](https://chatgpt.com/codex) | Access GPT-5 Codex models optimized for code generation and understanding. **Requires a ChatGPT Plus/Pro subscription.** | No manual key. Uses browser-based OAuth authentication for both CLI and Desktop. |
| [Databricks](https://www.databricks.com/)                                   | Unified data analytics and AI platform for building and deploying models.                                                                                                                                                 | `DATABRICKS_HOST`, `DATABRICKS_TOKEN` |
| [Docker Model Runner](https://docs.docker.com/ai/model-runner/)                             | Local models running in Docker Desktop or Docker CE with OpenAI-compatible API endpoints. **Because this provider runs locally, you must first [download a model](#local-llms).**                     | `OPENAI_HOST`, `OPENAI_BASE_PATH`   |
| [EmpirioLabs AI](https://empiriolabs.ai/)                                      | Frontier open and proprietary chat models (Qwen, DeepSeek, GLM, Kimi, MiniMax) through one OpenAI-compatible API with streaming. Catalog available at `https://api.empiriolabs.ai/v1/models`.        | `EMPIRIOLABS_API_KEY`                                                                                                                                                              |
| [Friendli AI](https://friendli.ai/)                                            | Friendli Model APIs provide instant access to a curated set of models, powered by a proprietary inference stack called [Friendli Engine](https://friendli.ai/why-friendliai) for high-performance, cost-efficient inference.               | `FRIENDLI_API_KEY`                                                                                                                                                                  |
| [FuturMix](https://futurmix.ai/)                                            | Unified AI gateway providing access to models from Anthropic, Google, OpenAI, and DeepSeek through an OpenAI-compatible API.                                                                          | `FUTURMIX_API_KEY`                                                                                                                                                                  |
| [Gemini](https://ai.google.dev/gemini-api/docs)                             | Advanced LLMs by Google with multimodal capabilities (text, images). Gemini 3 models support configurable [thinking levels](#gemini-3-thinking-levels).                                                                                                | `GOOGLE_API_KEY`, `GEMINI3_THINKING_LEVEL` (optional)                                                                                                                              |
| [GCP Vertex AI](https://cloud.google.com/vertex-ai)                         | Google Cloud's Vertex AI platform, supporting Gemini and Claude models. **Credentials must be [configured in advance](https://cloud.google.com/vertex-ai/docs/authentication).** Filters for allowed models by organization policy (if configured). | `GCP_PROJECT_ID`, `GCP_LOCATION` and optionally `GCP_MAX_RATE_LIMIT_RETRIES` (5), `GCP_MAX_OVERLOADED_RETRIES` (5), `GCP_INITIAL_RETRY_INTERVAL_MS` (5000), `GCP_BACKOFF_MULTIPLIER` (2.0), `GCP_MAX_RETRY_INTERVAL_MS` (320_000). |
| [GitHub Copilot](https://docs.github.com/en/copilot/using-github-copilot/ai-models) | Access to AI models from OpenAI, Anthropic, Google, and other providers through GitHub's Copilot infrastructure. **GitHub account with Copilot access required.** | No manual key. Uses [device flow authentication](#github-copilot-authentication) for both CLI and Desktop. |
| [Gondola](https://gondola-ai.com/guides)                                     | Pay-per-request inference via Venice AI, settled in USDC on Base. No subscription or minimum. Includes privacy-preserving TEE models (`e2ee-*`). OpenAI-compatible.                                                      | `GONDOLA_API_KEY`, `GONDOLA_HOST` (optional)                                                                                                                                        |
| [Groq](https://groq.com/)                                                   | High-performance inference hardware and tools for LLMs.                                                                                                                                                                   | `GROQ_API_KEY`                                                                                                                                                                      |
| [iFlytek Spark](https://www.xfyun.cn/doc/spark/HTTP%E8%B0%83%E7%94%A8%E6%96%87%E6%A1%A3.html) | iFlytek Spark (讯飞星火) models (4.0Ultra, generalv3.5, max-32k) via the OpenAI-compatible HTTP API. Best for chat: Spark needs `tool_calls_switch=true` (not injectable here) to return OpenAI-style tool calls. | `SPARK_API_PASSWORD` |
| [iFlytek Astron MaaS](https://maas.xfyun.cn/)                               | iFlytek Astron MaaS (讯飞星辰) hosting Spark X2, DeepSeek, GLM, Kimi, MiniMax, Qwen, and Astron coding models via an OpenAI-compatible API. Set `ASTRON_BASE_URL` to switch between the Token Plan and Coding Plan endpoints. | `ASTRON_API_KEY`, `ASTRON_BASE_URL` (optional) |
| [LiteLLM](https://docs.litellm.ai/docs/) | LiteLLM proxy supporting multiple models with automatic prompt caching and unified API access. | `LITELLM_HOST`, `LITELLM_BASE_PATH` (optional), `LITELLM_API_KEY` (optional), `LITELLM_CUSTOM_HEADERS` (optional), `LITELLM_TIMEOUT` (optional) |
| [LM Studio](https://lmstudio.ai/)                                          | Run local models with LM Studio's OpenAI-compatible server. **Because this provider runs locally, you must first [download a model](#local-llms).**                                                           | None required. Connects to local server at `localhost:1234` by default.                                                                                                             |
| [Meta](https://dev.meta.ai/)                                                | Meta's Model API, home of the Muse Spark models.                                                                                                                                | `META_MODEL_API_KEY`                                                                                                                                                                |
| [Mistral AI](https://mistral.ai/)                                           | Provides access to Mistral models including general-purpose models, specialized coding models (Codestral), and multimodal models (Pixtral).                                                                   | `MISTRAL_API_KEY`                                                                                                 |
| [NEAR AI Cloud](https://cloud.near.ai/)                                     | TEE-backed private inference through an OpenAI-compatible API with dynamic model discovery.                                                                                                                   | `NEARAI_API_KEY`                                                                                                                                                                  |
| [Novita AI](https://novita.ai/)                                             | 90+ open-source models with OpenAI-compatible API and competitive pricing. Supports Kimi K2.5, DeepSeek, GLM, MiniMax, Qwen, and more.                                                                       | `NOVITA_API_KEY`                                                                                                  |
| [Ollama](https://ollama.com/)                                               | Local model runner supporting Qwen, Llama, DeepSeek, and other open-source models. **Because this provider runs locally, you must first [download and run a model](#local-llms).**  | `OLLAMA_HOST`                                                                                                                                                                       |
| [Ollama Cloud](https://ollama.com/)                                         | Access hosted models on ollama.com via OpenAI-compatible API. Requires an Ollama account and API key.  | `OLLAMA_CLOUD_API_KEY`                                                                                                                                                                       |
| [OpenAI](https://platform.openai.com/api-keys)                              | Provides gpt-4o, o1, and other advanced language models. Also supports OpenAI-compatible endpoints (e.g., self-hosted LLaMA, vLLM, KServe). **o1-mini and o1-preview are not supported because goose uses tool calling.** | `OPENAI_API_KEY`, `OPENAI_HOST` (optional), `OPENAI_ORGANIZATION` (optional), `OPENAI_PROJECT` (optional), `OPENAI_CUSTOM_HEADERS` (optional)                                       |
| [OpenRouter](https://openrouter.ai/)                                        | API gateway for unified access to various models with features like rate-limiting management.                                                                                                                             | `OPENROUTER_API_KEY`, `OPENROUTER_HOST` (optional), `OPENROUTER_PARAMETERS` (optional)                                                                                              |
| [Perplexity](https://www.perplexity.ai/)                                    | Chat models with built-in real-time web search grounding. OpenAI-compatible chat completions API at `https://api.perplexity.ai`.                                                                                          | `PERPLEXITY_API_KEY`                                                                                                                                                                |
| [OVHcloud AI](https://www.ovhcloud.com/en/public-cloud/ai-endpoints/)       | Provides access to open-source models including Qwen, Llama, Mistral, and DeepSeek through AI Endpoints service.                                                       | `OVHCLOUD_API_KEY`                                                                                                                                                                  |
| [Ramalama](https://ramalama.ai/)                                            | Local model using native [OCI](https://opencontainers.org/) container runtimes, [CNCF](https://www.cncf.io/) tools, and supporting models as OCI artifacts. Ramalama API is a compatible alternative to Ollama and can be used with the goose Ollama provider. Supports Qwen, Llama, DeepSeek, and other open-source models. **Because this provider runs locally, you must first [download and run a model](#local-llms).**  | `OLLAMA_HOST`                                                                                                                                                                       |
| [Routstr](https://routstr.com/)                                             | OpenAI-compatible aggregator that fronts dozens of upstream providers (Anthropic, OpenAI, Google, DeepSeek, Llama, …) behind a single API. Authenticate with an `sk-...` bearer issued by your Routstr instance — payment is handled outside goose.                                                                                                                                                                       | `ROUTSTR_API_KEY`, `ROUTSTR_HOST` (optional, default `https://api.routstr.com`)                                                                                                     |
| [SayGM](https://saygm.com/)                                                 | TEE-backed private inference via an OpenAI-compatible API with dynamic model routing. Prices are determined at runtime per request.                                                                           | `SAYGM_API_KEY`                                                                                                   |
| [SaladCloud AI Gateway](https://salad.com/)                                 | OpenAI-compatible access to SaladCloud-hosted open-source models, including Qwen, Gemma, and others.                                                                                                          | `SALAD_CLOUD_API_KEY`                                                                                                                                                              |
| [Scaleway](https://www.scaleway.com/en/generative-apis/)                    | European cloud offering OpenAI-compatible access to models like Mistral, Qwen, and open-source weights. Ensures data residency and GDPR compliance.                                                                                                                                                                                                                                                                | `SCW_SECRET_KEY`      |
| [Snowflake](https://docs.snowflake.com/user-guide/snowflake-cortex/aisql#choosing-a-model) | Access the latest models using Snowflake Cortex services, including Claude models. **Requires a Snowflake account and programmatic access token (PAT)**.                                                     | `SNOWFLAKE_HOST`, `SNOWFLAKE_TOKEN`                                                                                                                                                                 |
| [VMware Tanzu Platform](https://techdocs.broadcom.com/us/en/vmware-tanzu/platform/ai-services/10-3/ai/index.html) | Enterprise-managed LLM access through AI Services on VMware Tanzu Platform. Models are fetched dynamically from the endpoint. | `TANZU_AI_API_KEY`, `TANZU_AI_ENDPOINT` |
| [Tetrate Agent Router Service](https://router.tetrate.ai)                   | Unified API gateway for AI models including Claude, Gemini, GPT, open-weight models, and others. Supports PKCE authentication flow for secure API key generation.                                                                                | `TETRATE_API_KEY`, `TETRATE_HOST` (optional)                                                                                                                                        |
| [TrustedRouter](https://trustedrouter.com)                                  | Models from OpenAI, Anthropic, Google, DeepSeek and others via TrustedRouter's OpenAI-compatible API, with per-request routing and failover.                                                                                                     | `TRUSTEDROUTER_API_KEY`                                                                                                                                                             |
| [Venice AI](https://venice.ai/home)                                         | Provides access to open source models like Llama, Mistral, and Qwen while prioritizing user privacy. **Requires an account and an [API key](https://docs.venice.ai/overview/guides/generating-api-key)**.                 | `VENICE_API_KEY`, `VENICE_HOST` (optional), `VENICE_BASE_PATH` (optional), `VENICE_MODELS_PATH` (optional)                                                                          |
| [Cerebras](https://cerebras.ai/)                                            | Fast inference on Cerebras wafer-scale engines with models like Llama, Qwen, and others.                                                                                                                                  | `CEREBRAS_API_KEY`                                                                                                                                                                  |
| [xAI](https://x.ai/)                                                        | Access to xAI's Grok models including grok-3, grok-3-mini, and grok-3-fast with 131,072 token context window.                                                                                                            | `XAI_API_KEY`, `XAI_HOST` (optional)                                                                                                                                                |

:::tip Prompt Caching for Claude Models
goose automatically enables Anthropic's [prompt caching](https://platform.claude.com/docs/en/build-with-claude/prompt-caching) when using Claude models via Anthropic, Amazon Bedrock, Databricks, OpenRouter, and LiteLLM providers. This adds `cache_control` markers to requests, which can reduce costs for longer conversations by caching frequently-used context. See the [provider implementations](https://github.com/aaif-goose/goose/tree/main/crates/goose/src/providers) for technical details.
:::

### CLI Providers

| Provider                                                                    | Description                                                                                                                                                                                                               | Requirements                                                                                                                                                                          |
|-----------------------------------------------------------------------------|---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|-------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| [Cursor Agent](https://docs.cursor.com/en/cli/overview) (`cursor-agent`)   | Uses Cursor's AI CLI tool with your Cursor subscription. Provides access to GPT-5, Claude 4, and other models through the cursor-agent command-line interface.                                              | cursor-agent CLI installed and authenticated                                                                                                         |

### ACP Providers

goose supports [Agent Client Protocol (ACP)](https://agentclientprotocol.com/) agents as providers. ACP providers pass goose extensions through to the agent as MCP servers.

| Provider                                                                    | Description                                                                                                                                                                                                               | Requirements                                                                                                                                                                          |
|-----------------------------------------------------------------------------|---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|-------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| [Claude ACP](https://github.com/agentclientprotocol/claude-agent-acp) (`claude-acp`) | Uses Claude Code via ACP. Passes goose extensions to the agent as MCP servers. | `npm install -g @agentclientprotocol/claude-agent-acp`, active Claude Code subscription |
| [Codex ACP](https://github.com/agentclientprotocol/codex-acp) (`codex-acp`) | Uses OpenAI Codex via ACP. Passes goose extensions to the agent as MCP servers. | `npm install -g @agentclientprotocol/codex-acp`, active ChatGPT Plus/Pro subscription or OpenAI API credits |

:::tip ACP Providers
See the [ACP Providers guide](/docs/guides/acp-providers) for detailed setup instructions.
:::

## Configure Provider and Model

To configure your chosen provider, see available options, or select a model, visit the `Models` tab in goose Desktop or run `goose configure` in the CLI.

<Tabs groupId="interface">
  <TabItem value="ui" label="goose Desktop" default>
  **First-time users:**

  On the welcome screen the first time you open goose, you have these options:

  <OnboardingProviderSetup />

  <Tabs groupId="setup">
    <TabItem value="apikey" label="Quick Setup" default>
    1. Choose `Quick Setup with API Key`.
    2. Enter your API key from your provider (for example, OpenAI, Anthropic, or Google).
    3. goose will automatically detect your provider and configure the connection.
    4. When setup is complete, you're ready to begin your first session.
    </TabItem>

    <TabItem value="chatgpt" label="ChatGPT Subscription">
    1. Choose `ChatGPT Subscription`.
    2. goose will open a browser window for you to sign in with the credentials of your active ChatGPT Plus or Pro subscription.
    3. Authorize goose to access your ChatGPT subscription.
    4. When you return to goose Desktop, you're ready to begin your first session.
    </TabItem>
    <TabItem value="tetrate" label="Agent Router">
    We recommend new users start with Agent Router by Tetrate. Tetrate provides access to multiple AI models with built-in rate limiting and automatic failover.

    :::info Free Credits Offer
    You'll receive $10 in free credits the first time you automatically authenticate with Tetrate through goose. This offer is available to both new and existing Tetrate users.
    :::
    1. Choose `Agent Router by Tetrate`.
    2. goose will open a browser window for you to authenticate with Tetrate, or create a new account if you don't have one already.
    3. When you return to goose Desktop, you're ready to begin your first session.
    </TabItem>

    <TabItem value="openrouter" label="OpenRouter">
    1. Choose `Automatic setup with OpenRouter`.
    2. goose will open a browser window for you to authenticate with OpenRouter, or create a new account if you don't have one already.
    3. When you return to the goose Desktop, you're ready to begin your first session.
    </TabItem>

    <TabItem value="others" label="Other Providers">
    1. If you have a specific provider you want to use with goose, and an API key from that provider, choose `Other Providers`.
    2. Find the provider of your choice and click its `Configure` button. If you don't see your provider in the list, click `Add Custom Provider` at the bottom of the window to [configure a custom provider](#configure-custom-provider).
    3. Depending on your provider, you'll need to input your API Key, API Host, or other optional [parameters](#available-providers). Click the `Submit` button to authenticate and begin your first session.

    :::info Ollama Model Detection
    For Ollama users, all locally installed models display automatically in the model selection dropdown.
    :::

    </TabItem>
  </Tabs>
  **To update your LLM provider and API key:**
  1. Click the <PanelLeft className="inline" size={16} /> button in the top-left to open the sidebar
  2. Click the `Settings` button on the sidebar
  3. Click the `Models` tab
  4. Click `Configure providers`
  5. Click your provider in the list
  6. Add your API key and other required configurations, then click `Submit`

  **To change your current model:**
  1. Click the <PanelLeft className="inline" size={16} /> button in the top-left to open the sidebar
  2. Click the `Settings` button on the sidebar
  3. Click the `Models` tab
  4. Click `Switch models`
  5. Choose from your configured providers in the dropdown, or select `Use other provider` to configure a new one
  6. Select a model from the available options, or choose `Use custom model` to enter a specific model name
  7. Click `Select model` to confirm your choice

  :::tip Shortcut
  For faster access, click your current model name at the bottom of the app and choose `Change Model`.
  :::

  **To start over with provider and model configuration:**
  1. Click the <PanelLeft className="inline" size={16} /> button in the top-left to open the sidebar
  2. Click the `Settings` button on the sidebar
  3. Click the `Models` tab
  4. Click `Reset Provider and Model` to clear your current settings and return to the welcome screen
  </TabItem>
  <TabItem value="cli" label="goose CLI">
    1. In your terminal, run the following command:

       ```sh
       goose configure
       ```

    2. Select `Configure Providers` from the menu and press `Enter`.

       ```
       ┌   goose-configure
       │
       ◆  What would you like to configure?
       // highlight-start
       │  ● Configure Providers (Change provider or update credentials)
       // highlight-end
       │  ○ Custom Providers
       │  ○ Add Extension
       │  ○ Toggle Extensions
       │  ○ Remove Extension
       │  ○ goose Settings
       └
       ```
    3. Choose a model provider and press `Enter`. Use the arrow keys (↑/↓) to move through the options, or start typing to filter the list.

       ```
       ┌   goose-configure
       │
       ◇  What would you like to configure?
       │  Configure Providers
       │
       ◆  Which model provider should we use?
       │  ○ Amazon Bedrock
       │  ○ Amazon SageMaker TGI
       // highlight-start
       │  ● Anthropic (Claude and other models from Anthropic)
       // highlight-end
       │  ○ Azure OpenAI
       │  ○ Claude Code CLI
       │  ○ ...
       └
       ```
    4. Enter your API key (and any other configuration details) when prompted.

       ```
       ┌   goose-configure
       │
       ◇  What would you like to configure?
       │  Configure Providers
       │
       ◇  Which model provider should we use?
       │  Anthropic
       │
       ◆  Provider Anthropic requires ANTHROPIC_API_KEY, please enter a value
       // highlight-start
       │  ▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪
       // highlight-end
       └
       ```

       If you're just changing models, skip any prompts to update the provider configuration.

    5. Enter your desired `ANTHROPIC_HOST` or press `Enter` to use the default.

       ```
       ◆  Provider Anthropic requires ANTHROPIC_HOST, please enter a value
       // highlight-start
       │  https://api.anthropic.com (default)
       // highlight-end
       ```
    6. Choose the model you want to use. Depending on the provider, you can:
       - Select the model from a list
       - Search for the model by name
       - Enter the model name directly

       ```
       │
       ◇  Model fetch complete
       │
       ◇  Select a model:
       // highlight-start
       │  claude-sonnet-4-5 (default)
       // highlight-end
       │
       ◒  Checking your configuration...
       └  Configuration saved successfully
       ```

       This change takes effect the next time you start a session.

  :::note
  `goose configure` doesn't support entering custom model names. To use a model not in the provider's list, use goose Desktop or edit the `GOOSE_MODEL` variable in your [`config.yaml`](/docs/guides/config-files) directly.
  :::

  :::tip
  Set the model for an individual session using the [`run` command](/docs/guides/goose-cli-commands#run-options):

  ```bash
  goose run --model claude-sonnet-4-0 -t "initial prompt"
  ```
  :::

  </TabItem>
</Tabs>

### Using Custom OpenAI Endpoints

The built-in OpenAI provider can connect to OpenAI's official API (`api.openai.com`) or any OpenAI-compatible endpoint, such as:
- Self-hosted LLMs (e.g., LLaMA, Mistral) using vLLM or KServe
- Private OpenAI-compatible API servers
- Enterprise deployments requiring data governance and security compliance
- OpenAI API proxies or gateways

:::tip Custom Provider Option
Need to connect to multiple OpenAI-compatible endpoints? [Configure custom providers](#configure-custom-provider) instead for easier switching and better organization, as well as custom naming and shareable configurations.
:::

:::note Pointing at a LiteLLM proxy
You can reach a [LiteLLM](https://docs.litellm.ai/) proxy in either of two ways—pick one, don't mix them:

- Use the **OpenAI provider**: set `OPENAI_HOST` to your proxy's root (no trailing path) and `OPENAI_BASE_PATH` to the path it serves (usually `v1/chat/completions`). A `404` usually means `OPENAI_BASE_PATH` is wrong for your proxy. A `401` with `No api key passed in` is a different problem—the API key is not being loaded (for example, a key placed in `config.yaml`, which is ignored); see [Provider API keys and `config.yaml`](/docs/guides/config-files#security-considerations).
- Use the dedicated **LiteLLM provider**, which is configured with its own `LITELLM_HOST`, `LITELLM_BASE_PATH`, and `LITELLM_API_KEY` variables instead of the `OPENAI_*` ones.
:::

#### Configuration Parameters

| Parameter | Required | Description |
|-----------|----------|-------------|
| `OPENAI_API_KEY` | Yes | Authentication key for the API |
| `OPENAI_HOST` | No | Custom endpoint URL (defaults to api.openai.com) |
| `OPENAI_BASE_PATH` | No | Request path appended to the host (defaults to `v1/chat/completions`). Set this when your endpoint serves the chat completions API at a different path—most proxies expect `v1/chat/completions`, but some are mounted at `chat/completions` (no `v1`). |
| `OPENAI_ORGANIZATION` | No | Organization ID for usage tracking and governance |
| `OPENAI_PROJECT` | No | Project identifier for resource management |
| `OPENAI_CUSTOM_HEADERS` | No | Additional headers to include in the request. Can be set via environment variable, configuration file, or CLI, in the format `HEADER_A=VALUE_A,HEADER_B=VALUE_B`. |
| `OPENAI_STORE` | No | Whether to persist the generated Responses API response for later retrieval via API. Defaults to `false`. |

#### Example Configurations

<Tabs groupId="deployment">
  <TabItem value="vllm" label="vLLM Self-Hosted" default>
    If you're running LLaMA or other models using vLLM with OpenAI compatibility:
    ```sh
    OPENAI_HOST=https://your-vllm-endpoint.internal
    OPENAI_API_KEY=your-internal-api-key
    ```
  </TabItem>
  <TabItem value="kserve" label="KServe Deployment">
    For models deployed on Kubernetes using KServe:
    ```sh
    OPENAI_HOST=https://kserve-gateway.your-cluster
    OPENAI_API_KEY=your-kserve-api-key
    OPENAI_ORGANIZATION=your-org-id
    OPENAI_PROJECT=ml-serving
    ```
  </TabItem>
  <TabItem value="enterprise" label="Enterprise OpenAI">
    For enterprise OpenAI deployments with governance:
    ```sh
    OPENAI_API_KEY=your-api-key
    OPENAI_ORGANIZATION=org-id123
    OPENAI_PROJECT=compliance-approved
    ```
  </TabItem>
  <TabItem value="custom-headers" label="Custom Headers">
    For OpenAI-compatible endpoints that require custom headers:
    ```sh
    OPENAI_API_KEY=your-api-key
    OPENAI_ORGANIZATION=org-id123
    OPENAI_PROJECT=compliance-approved
    OPENAI_CUSTOM_HEADERS="X-Header-A=abc,X-Header-B=def"
    ```
  </TabItem>
</Tabs>

#### Setup Instructions

<Tabs groupId="interface">
  <TabItem value="ui" label="goose Desktop" default>
    1. Click the <PanelLeft className="inline" size={16} /> button in the top-left to open the sidebar
    2. Click the `Settings` button on the sidebar
    3. Click the `Models` tab
    4. Click `Configure providers`
    5. Click `OpenAI` in the provider list
    6. Fill in your configuration details:
       - API Key (required)
       - Host URL (for custom endpoints)
       - Organization ID (for usage tracking)
       - Project (for resource management)
    7. Click `Submit`
  </TabItem>
  <TabItem value="cli" label="goose CLI">
    1. Run `goose configure`
    2. Select `Configure Providers`
    3. Choose `OpenAI` as the provider
    4. Enter your configuration when prompted:
       - API key
       - Host URL (if using custom endpoint)
       - Organization ID (if using organization tracking)
       - Project identifier (if using project management)
  </TabItem>
</Tabs>

:::tip Enterprise Deployment
For enterprise deployments, you can pre-configure these values using environment variables or configuration files to ensure consistent governance across your organization.
:::

## Configure Custom Provider

Create custom providers to connect to services that aren't [already supported](#available-providers) or customize how you connect to them. Custom providers appear in goose's provider list and can be selected like any other provider.

**Benefits:**
- **Multiple endpoints**: Switch between different services (e.g., vLLM, corporate proxy, OpenAI)
- **Pre-configured models**: Store a list of preferred models
- **Shareable configuration**: JSON files can be shared across teams or checked into repos
- **Custom naming**: Show "Corporate API" instead of "OpenAI" in the UI
- **Separate credentials**: Assign each provider its own API key

Custom providers must use OpenAI, Anthropic, or Ollama compatible API formats. They can include custom headers for additional authentication, API keys, tokens, or tenant identifiers. Each custom provider maps to a JSON configuration file.

**To add a custom provider:**
<Tabs groupId="interface">
  <TabItem value="ui" label="goose Desktop" default>
    1. Click the <PanelLeft className="inline" size={16} /> button in the top-left to open the sidebar
    2. Click the `Settings` button on the sidebar
    3. Click the `Models` tab
    4. Click `Configure providers`
    5. Click `Add Custom Provider` at the bottom of the window
    6. Fill in the provider details:
       - **Provider Type**:
         - `OpenAI Compatible` (most common)
         - `Anthropic Compatible`
         - `Ollama Compatible`
       - **Display Name**: A friendly name for the provider
       - **API URL**: The base URL of the API endpoint
       - **Authentication**:
         - **API Key**: The API key, which is accessed using a custom environment variable and stored in the keychain (or `secrets.yaml` if the keyring is disabled or cannot be accessed)
            - For providers that don't require authorization (e.g., local models like Ollama, vLLM, or internal APIs), uncheck the **"This provider requires an API key"** checkbox
       - **Available Models**: Comma-separated list of available model names
       - **Streaming Support**: Whether the API supports streaming responses (click to toggle)
    7. Click `Create Provider`

    :::info Custom Headers
    Currently, custom headers can't be defined in goose Desktop. As a workaround, edit the provider configuration file after creation.
    :::

  </TabItem>
  <TabItem value="cli" label="goose CLI">
    1. In your terminal, run the following command:

       ```sh
       goose configure
       ```

    2. Select `Custom Providers`. Use the arrow keys (↑/↓) to move through the options.

       ```sh
       ┌   goose-configure
       │
       ◆  What would you like to configure?
       │  ○ Configure Providers
       // highlight-start
       │  ● Custom Providers (Add custom provider with compatible API)
       // highlight-end
       │  ○ Add Extension
       │  ○ Toggle Extensions
       │  ○ Remove Extension
       │  ○ goose Settings
       └
       ```

    3. Select `Add A Custom Provider`

       ```sh
       ┌   goose-configure
       │
       ◇  What would you like to configure?
       │  Custom Providers
       │
       ◆  What would you like to do?
       // highlight-start
       │  ● Add A Custom Provider (Add a new OpenAI/Anthropic/Ollama compatible Provider)
       // highlight-end
       │  ○ Remove Custom Provider
       └
       ```

    4. Follow the prompts to enter the provider details:
       - **API Type**:
         - `OpenAI Compatible` (most common)
         - `Anthropic Compatible`
         - `Ollama Compatible`
       - **Name**: A friendly name for the provider
       - **API URL**: The base URL of the API endpoint
       - **Authentication Required**: Answer "Yes" if your provider needs an API key, or "No" if authentication is not required
         - If Yes: Choose how goose should obtain the credential:
           - **Static API key**: You'll be prompted to enter your **API Key** (stored securely in the keychain, or in `secrets.yaml` if the keyring is disabled or cannot be accessed)
           - **Command (refreshable)**: You'll be prompted for a **command** (and optional arguments) that goose runs to fetch the credential, plus a **refresh interval** in seconds. Use this for short-lived credentials issued by an IdP or key vault, so goose can refresh them automatically instead of requiring a restart when they expire. See [Command-Based Authentication](#command-based-authentication) below.
         - If No: The API key prompt is skipped
       - **Available Models**: Comma-separated list of available model names
       - **Streaming Support**: Whether the API supports streaming responses
       - **Custom Headers**: Any additional header names and values

    :::info Custom Headers
    Currently, custom headers can only be defined for OpenAI compatible providers in the CLI. For Anthropic or Ollama compatible providers, edit the provider configuration file after creation.
    :::

  </TabItem>
  <TabItem value="config" label="Config File">

    First create a JSON file in the `custom_providers` directory:
    - macOS/Linux: `~/.config/goose/custom_providers/`
    - Windows: `%APPDATA%\Block\goose\config\custom_providers\`

    Example `custom_corp_api.json` configuration file:
    ```json
    {
      "name": "custom_corp_api",
      "engine": "openai",
      "display_name": "Corporate API",
      "description": "Custom Corporate API provider",
      "api_key_env": "CUSTOM_CORP_API_API_KEY",
      "base_url": "https://api.company.com/v1/chat/completions",
      "models": [
        {
          "name": "gpt-4o",
          "context_limit": 128000
        },
        {
          "name": "gpt-3.5-turbo",
          "context_limit": 16385
        }
      ],
      "headers": {
        "x-origin-client-id": "YOUR_CLIENT_ID",
        "x-origin-secret": "YOUR_SECRET_VALUE"
      },
      "supports_streaming": true,
      "requires_auth": true
    }
    ```

    Then use the `api_key_env` to set the key for your session. For example:
    ```bash
    export CUSTOM_CORP_API_API_KEY="your-api-key"
    goose session start --provider custom_corp_api
    ```

    :::tip Keychain Key Storage
    If you want to store the API key in the `goose` keychain, update the provider in goose Desktop and enter the key. This provides secure, persistent storage and allows goose to connect natively to the provider.
    :::

  </TabItem>
</Tabs>

### Command-Based Authentication

Instead of a static `api_key_env`, a custom provider can be configured to run a command to obtain its credential. This is useful for short-lived credentials issued by an IdP or key vault: goose re-runs the command to refresh the credential instead of requiring a restart when it expires.

Add an `auth` object to the provider's JSON configuration in place of `api_key_env` (the two are mutually exclusive):

```json
{
  "name": "custom_corp_api",
  "engine": "openai",
  "display_name": "Corporate API",
  "base_url": "https://api.company.com/v1/chat/completions",
  "models": [{ "name": "gpt-4o", "context_limit": 128000 }],
  "requires_auth": true,
  "auth": {
    "command": "/path/to/get-token.sh",
    "args": [],
    "refresh_interval": 3600,
    "timeout_seconds": 10
  }
}
```

- **`command`**: The executable to run. It is spawned directly, without a shell — none of `command`/`args` are shell-interpolated. If your script needs shell features (pipes, variable expansion), invoke an interpreter explicitly, e.g. `"command": "/bin/bash", "args": ["-c", "..."]`. A bare name (no path separator, e.g. `"get-token"`) is looked up on `PATH`; a relative path (e.g. `"./scripts/get-token.sh"`) is resolved against `cwd`.
- **`args`** (optional): Arguments passed to `command`.
- **`refresh_interval`** (optional, defaults to `3600`): How long, in seconds, a fetched credential is cached before the command is re-run. Set to `0` to disable proactive refresh entirely — the command then only reruns reactively, after the provider's API rejects a request with an auth error.
- **`timeout_seconds`** (optional, defaults to `10`): How long to wait for the command before treating it as failed.
- **`cwd`** (optional): Working directory for the command, and the base a relative `command` path is resolved against. Defaults to goose's current directory.

The command's trimmed standard output is used as the credential. It must exit successfully and print a non-empty value; on failure, goose surfaces an error rather than silently reusing a stale credential. The command inherits goose's full environment, since the same user configures goose and writes the script.

<Tabs groupId="interface">
  <TabItem value="cli" label="goose CLI" default>

    In `goose configure`, choose **Command (refreshable)** when prompted for how to obtain credentials for a custom provider (see [above](#configure-custom-provider)).

  </TabItem>
  <TabItem value="config" label="Config File">

    Add the `auth` object shown above to the provider's JSON file instead of setting `api_key_env`.

  </TabItem>
</Tabs>

**To update a custom provider:**

<Tabs groupId="interface">
  <TabItem value="ui" label="goose Desktop" default>
    1. Click the <PanelLeft className="inline" size={16} /> button in the top-left to open the sidebar
    2. Click the `Settings` button on the sidebar
    3. Click the `Models` tab
    4. Click `Configure providers`
    5. Click on your custom provider in the list
    6. Update the fields you want to change
    7. Click `Update Provider`

  </TabItem>
  <TabItem value="cli" label="goose CLI">

    1. In your terminal, run the following command:

       ```sh
       goose configure
       ```

    2. Select `Configure Providers` from the menu and press `Enter`.

       ```sh
       ┌   goose-configure
       │
       ◆  What would you like to configure?
       // highlight-start
       │  ● Configure Providers (Change provider or update credentials)
       // highlight-end
       │  ○ Custom Providers
       │  ○ Add Extension
       │  ○ Toggle Extensions
       │  ○ Remove Extension
       │  ○ goose Settings
       └
       ```

    3. Select the custom provider you want to update and press `Enter`. Use the arrow keys (↑/↓) to move through the options, or start typing to filter the list.

       ```sh
       ┌   goose-configure
       │
       ◇  What would you like to configure?
       │  Configure Providers
       │
       ◆  Which model provider should we use?
       │  ○ Amazon Bedrock
       │  ○ Amazon SageMaker TGI
       │  ○ Anthropic
       │  ○ Azure OpenAI
       │  ○ Claude Code CLI
       // highlight-start
       │  ● Corporate API (Custom Corporate API provider)
       // highlight-end
       │  ○ Cursor Agent
       │  ○ ...
       └
       ```

    4. Follow the prompts to update the fields.

  </TabItem>
  <TabItem value="config" label="Config File">

    Open the custom provider configuration file in the `custom_providers` directory:
    - macOS/Linux: `~/.config/goose/custom_providers/`
    - Windows: `%APPDATA%\Block\goose\config\custom_providers\`

    Update the fields you want to change and save your changes.
  </TabItem>
</Tabs>

Your changes are available in your next goose session.

**To remove a custom provider:**

<Tabs groupId="interface">
  <TabItem value="ui" label="goose Desktop" default>
    1. Click the <PanelLeft className="inline" size={16} /> button in the top-left to open the sidebar
    2. Click the `Settings` button on the sidebar
    3. Click the `Models` tab
    4. Click `Configure providers`
    5. Click on your custom provider in the list
    6. Click `Delete Provider`
    7. Confirm that you want to permanently remove the custom provider and its stored API key (if applicable) by clicking `Confirm Delete`

  </TabItem>
  <TabItem value="cli" label="goose CLI">

    1. In your terminal, run the following command:

       ```sh
       goose configure
       ```

    2. Select `Custom Providers`. Use the arrow keys (↑/↓) to move through the options.

       ```sh
       ┌   goose-configure
       │
       ◆  What would you like to configure?
       │  ○ Configure Providers
       // highlight-start
       │  ● Custom Providers (Add custom provider with compatible API)
       // highlight-end
       │  ○ Add Extension
       │  ○ Toggle Extensions
       │  ○ Remove Extension
       │  ○ goose Settings
       └
       ```

    3. Select `Remove Custom Provider`.

       ```sh
       ┌   goose-configure
       │
       ◇  What would you like to configure?
       │  Custom Providers
       │
       ◆  What would you like to do?
       │  ○ Add A Custom Provider
       // highlight-start
       │  ● Remove Custom Provider (Remove an existing custom provider)
       // highlight-end
       └
       ```

    4. Select the custom provider you want to remove.

    The provider configuration file is removed from the `custom_providers` directory and the key is removed from the keychain.

  </TabItem>
  <TabItem value="config" label="Config File">

    :::tip
    If the provider's API key is stored in the keychain, use goose CLI to remove the custom provider. This also removes the stored API key.
    :::

    Delete the custom provider configuration file in the `custom_providers` directory:
    - macOS/Linux: `~/.config/goose/custom_providers/`
    - Windows: `%APPDATA%\Block\goose\config\custom_providers\`

  </TabItem>
</Tabs>

## Using goose for Free

goose is a free and open source AI agent that you can start using right away, but not all supported [LLM Providers][providers] provide a free tier.

Below, we outline a couple of free options and how to get started with them.

:::warning Limitations
These free options are a great way to get started with goose and explore its capabilities. However, you may need to upgrade your LLM for better performance.
:::


### Groq
Groq provides free access to open source (open weight) models with high-speed inference. To use Groq with goose, you need an API key from [Groq Console](https://console.groq.com/keys).

Groq offers several open source models that support tool calling, including:
- **moonshotai/kimi-k2-instruct-0905** - Mixture-of-Experts model with 1 trillion parameters, optimized for agentic intelligence and tool use
- **qwen/qwen3-32b** - 32.8 billion parameter model with advanced reasoning and multilingual capabilities
- **llama-3.3-70b-versatile** - Meta's Llama 3.3 model for versatile applications
- **llama-3.1-8b-instant** - Meta's Llama 3.1 model for fast inference

For the complete list of supported Groq models, see [groq.json](https://github.com/aaif-goose/goose/blob/main/crates/goose/src/providers/declarative/groq.json).

To set up Groq with goose, follow these steps:

<Tabs groupId="interface">
  <TabItem value="ui" label="goose Desktop" default>
  **To update your LLM provider and API key:**

    1. Click the <PanelLeft className="inline" size={16} /> button in the top-left to open the sidebar.
    2. Click the `Settings` button on the sidebar.
    3. Click the `Models` tab.
    4. Click `Configure Providers`
    5. Choose `Groq` as provider from the list.
    6. Click `Configure`, enter your API key, and click `Submit`.
    7. Select the Groq model of your choice.

  </TabItem>
  <TabItem value="cli" label="goose CLI">
    1. Run:
    ```sh
    goose configure
    ```
    2. Select `Configure Providers` from the menu.
    3. Follow the prompts to choose `Groq` as the provider.
    4. Enter your API key when prompted.
    5. Select the Groq model of your choice.
  </TabItem>
</Tabs>

### EmpirioLabs AI
[EmpirioLabs AI](https://empiriolabs.ai/) provides access to frontier open and proprietary chat models through a single OpenAI-compatible API with streaming. To use EmpirioLabs with goose, you need an API key from [EmpirioLabs](https://platform.empiriolabs.ai/dashboard/api-keys).

EmpirioLabs offers models that support tool calling, including:
- **qwen3-7-plus** - Qwen3.7 Plus with a 1M context window
- **qwen3-7-max** - Qwen3.7 Max with a 1M context window
- **deepseek-v4-pro** - DeepSeek V4 Pro with a 1M context window
- **deepseek-v4-flash** - DeepSeek V4 Flash with a 1M context window
- **glm-5-1** - GLM-5.1 with a 202K context window
- **kimi-k2-7-code** - Kimi K2.7 Code with a 256K context window
- **minimax-m3** - MiniMax M3 with a 524K context window

The full live catalog is available at `https://api.empiriolabs.ai/v1/models`. For the complete list of EmpirioLabs models configured in goose, see [empiriolabs.json](https://github.com/aaif-goose/goose/blob/main/crates/goose/src/providers/declarative/empiriolabs.json). For more details, see the [EmpirioLabs documentation](https://docs.empiriolabs.ai).

To set up EmpirioLabs with goose, follow these steps:

<Tabs groupId="interface">
  <TabItem value="ui" label="goose Desktop" default>
  **To update your LLM provider and API key:**

    1. Click the <PanelLeft className="inline" size={16} /> button in the top-left to open the sidebar.
    2. Click the `Settings` button on the sidebar.
    3. Click the `Models` tab.
    4. Click `Configure Providers`
    5. Choose `EmpirioLabs AI` as provider from the list.
    6. Click `Configure`, enter your API key, and click `Submit`.
    7. Select the EmpirioLabs model of your choice.

  </TabItem>
  <TabItem value="cli" label="goose CLI">
    1. Run:
    ```sh
    goose configure
    ```
    2. Select `Configure Providers` from the menu.
    3. Follow the prompts to choose `EmpirioLabs AI` as the provider.
    4. Enter your API key when prompted.
    5. Select the EmpirioLabs model of your choice.
  </TabItem>
</Tabs>

### FuturMix
[FuturMix](https://futurmix.ai/) is a unified AI gateway providing access to models from Anthropic, Google, OpenAI, and DeepSeek through an OpenAI-compatible API. To use FuturMix with goose, you need an API key from [FuturMix](https://futurmix.ai/).

FuturMix offers models that support tool calling, including:
- **claude-sonnet-4-20250514** - Anthropic Claude Sonnet 4 with 200K context
- **gpt-4o** - OpenAI GPT-4o with 128K context
- **gemini-2.5-pro** - Google Gemini 2.5 Pro with 1M context
- **deepseek-chat** - DeepSeek V3 with 131K context
- **claude-haiku-4-20250514** - Anthropic Claude Haiku 4 with 200K context

For the complete list of supported FuturMix models, see [futurmix.json](https://github.com/aaif-goose/goose/blob/main/crates/goose/src/providers/declarative/futurmix.json).

To set up FuturMix with goose, follow these steps:

<Tabs groupId="interface">
  <TabItem value="ui" label="goose Desktop" default>
  **To update your LLM provider and API key:**

    1. Click the <PanelLeft className="inline" size={16} /> button in the top-left to open the sidebar.
    2. Click the `Settings` button on the sidebar.
    3. Click the `Models` tab.
    4. Click `Configure Providers`
    5. Choose `FuturMix` as provider from the list.
    6. Click `Configure`, enter your API key, and click `Submit`.
    7. Select the FuturMix model of your choice.

  </TabItem>
  <TabItem value="cli" label="goose CLI">
    1. Run:
    ```sh
    goose configure
    ```
    2. Select `Configure Providers` from the menu.
    3. Follow the prompts to choose `FuturMix` as the provider.
    4. Enter your API key when prompted.
    5. Select the FuturMix model of your choice.
  </TabItem>
</Tabs>

### Novita AI
[Novita AI](https://novita.ai/) provides access to 90+ open-source models via an OpenAI-compatible API with competitive pricing. To use Novita AI with goose, you need an API key from [Novita AI](https://novita.ai/settings#key-management).

Novita AI offers many models that support tool calling, including:
- **moonshotai/kimi-k2.5** - Moonshot's latest model with 262K context window
- **minimax/minimax-m2.7** - MiniMax M2.7 with 205K context
- **zai-org/glm-5.1** - Zhipu's GLM-5.1 with 205K context
- **deepseek/deepseek-v3.2** - DeepSeek V3.2 with 164K context
- **google/gemma-4-31b-it** - Google Gemma 4 31B with 262K context

For the complete list of supported Novita AI models, see [novita.json](https://github.com/aaif-goose/goose/blob/main/crates/goose/src/providers/declarative/novita.json).

To set up Novita AI with goose, follow these steps:

<Tabs groupId="interface">
  <TabItem value="ui" label="goose Desktop" default>
  **To update your LLM provider and API key:**

    1. Click the <PanelLeft className="inline" size={16} /> button in the top-left to open the sidebar.
    2. Click the `Settings` button on the sidebar.
    3. Click the `Models` tab.
    4. Click `Configure Providers`
    5. Choose `Novita AI` as provider from the list.
    6. Click `Configure`, enter your API key, and click `Submit`.
    7. Select the Novita AI model of your choice.

  </TabItem>
  <TabItem value="cli" label="goose CLI">
    1. Run:
    ```sh
    goose configure
    ```
    2. Select `Configure Providers` from the menu.
    3. Follow the prompts to choose `Novita AI` as the provider.
    4. Enter your API key when prompted.
    5. Select the Novita AI model of your choice.
  </TabItem>
</Tabs>

### Routstr
[Routstr](https://routstr.com/) is an OpenAI-compatible aggregator that fronts dozens of upstream providers behind a single API. Payment is handled by the Routstr instance itself, so all goose needs is the `sk-...` bearer that instance issues you. To use Routstr with goose, pick an instance (the default is `https://api.routstr.com`) and obtain an API key from its payment flow.

Routstr aggregates models from many upstream providers, including:
- **claude-opus-4.7** — Anthropic's Claude opus 4.7
- **deepseek-v4-pro** — DeepSeek V4 Pro
- **gemini-3.1-pro-preview** — gemini-3.1 Pro Preview

`/v1/models` is queried at configure time, so the full catalogue your Routstr instance exposes is available in the model picker. For the static defaults shipped with goose, see [routstr.json](https://github.com/aaif-goose/goose/blob/main/crates/goose/src/providers/declarative/routstr.json).

To set up Routstr with goose, follow these steps:

<Tabs groupId="interface">
  <TabItem value="ui" label="goose Desktop" default>
  **To update your LLM provider and API key:**

    1. Click the <PanelLeft className="inline" size={16} /> button in the top-left to open the sidebar.
    2. Click the `Settings` button on the sidebar.
    3. Click the `Models` tab.
    4. Click `Configure Providers`
    5. Choose `Routstr` as provider from the list.
    6. Click `Configure`, enter your `ROUTSTR_API_KEY` (and optionally override `ROUTSTR_HOST` to point at a different Routstr instance), and click `Submit`.
    7. Select the Routstr model of your choice.

  </TabItem>
  <TabItem value="cli" label="goose CLI">
    1. Run:
    ```sh
    goose configure
    ```
    2. Select `Configure Providers` from the menu.
    3. Follow the prompts to choose `Routstr` as the provider.
    4. Enter your API key when prompted (and optionally override `ROUTSTR_HOST`).
    5. Select the Routstr model of your choice.
  </TabItem>
</Tabs>

### SayGM
[SayGM](https://saygm.com/) provides TEE-backed private inference via an OpenAI-compatible API. Model routing and prices are determined at runtime per request. To use SayGM with goose, you need an API key from [SayGM](https://saygm.com/).

SayGM supports many models, including:
- **Qwen/Qwen3-235B-A22B-Thinking-2507-TEE** — Qwen3 235B thinking model (TEE-backed)
- **deepseek-ai/DeepSeek-V3.2-TEE** — DeepSeek V3.2 (TEE-backed)
- **moonshotai/Kimi-K3-TEE** — Kimi K3 (TEE-backed)
- **zai-org/GLM-5.2-TEE** — GLM-5.2 (TEE-backed)

`/v1/models` is queried at configure time, so the full catalogue SayGM exposes is available in the model picker.

To set up SayGM with goose, follow these steps:

<Tabs groupId="interface">
  <TabItem value="ui" label="goose Desktop" default>
  **To update your LLM provider and API key:**

    1. Click the <PanelLeft className="inline" size={16} /> button in the top-left to open the sidebar.
    2. Click the `Settings` button on the sidebar.
    3. Click the `Models` tab.
    4. Click `Configure Providers`
    5. Choose `SayGM` as provider from the list.
    6. Click `Configure`, enter your API key, and click `Submit`.
    7. Select the SayGM model of your choice.

  </TabItem>
  <TabItem value="cli" label="goose CLI">
    1. Run:
    ```sh
    goose configure
    ```
    2. Select `Configure Providers` from the menu.
    3. Follow the prompts to choose `SayGM` as the provider.
    4. Enter your API key when prompted.
    5. Select the SayGM model of your choice.
  </TabItem>
</Tabs>

### Google Gemini
Google Gemini provides a free tier. To start using the Gemini API with goose, you need an API Key from [Google AI studio](https://aistudio.google.com/app/apikey).

To set up Google Gemini with goose, follow these steps:

<Tabs groupId="interface">
  <TabItem value="ui" label="goose Desktop" default>
  **To update your LLM provider and API key:**

    1. Click the <PanelLeft className="inline" size={16} /> button in the top-left to open the sidebar.
    2. Click the `Settings` button on the sidebar.
    3. Click the `Models` tab.
    4. Click `Configure Providers`
    5. Choose `Google Gemini` as provider from the list.
    6. Click `Configure`, enter your API key, and click `Submit`.

  </TabItem>
  <TabItem value="cli" label="goose CLI">
    1. Run:
    ```sh
    goose configure
    ```
    2. Select `Configure Providers` from the menu.
    3. Follow the prompts to choose `Google Gemini` as the provider.
    4. Enter your API key when prompted.
    5. Enter the Gemini model of your choice.

    ```
    ┌   goose-configure
    │
    ◇ What would you like to configure?
    │ Configure Providers
    │
    ◇ Which model provider should we use?
    │ Google Gemini
    │
    ◇ Provider Google Gemini requires GOOGLE_API_KEY, please enter a value
    │▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪
    │
    ◇ Enter a model from that provider:
    │ gemini-2.0-flash-exp
    │
    ◇ Hello! You're all set and ready to go, feel free to ask me anything!
    │
    └ Configuration saved successfully
    ```
  </TabItem>
</Tabs>


### Local LLMs

goose is a local AI agent, and by using a local LLM, you keep your data private, maintain full control over your environment, and can work entirely offline without relying on cloud access. However, please note that local LLMs require a bit more set up before you can use one of them with goose.

:::warning Limited Support for models without tool calling
goose extensively uses tool calling, so models without it can only do chat completion. If using models without tool calling, all goose [extensions must be disabled](/docs/getting-started/using-extensions#enablingdisabling-extensions).
:::

Here are some local providers we support:

<Tabs groupId="local-llms">
  <TabItem value="ollama" label="Ollama" default>
    <Tabs groupId="ollama-models">
      <TabItem value="ramalala" label="Ramalala">
        1. [Download Ramalama](https://github.com/containers/ramalama?tab=readme-ov-file#install).
        2. In a terminal, run any Ollama [model supporting tool-calling](https://ollama.com/search?c=tools) or [GGUF format HuggingFace Model](https://huggingface.co/search/full-text?q=%22tools+support%22+%2B+%22gguf%22&type=model):

          The `--runtime-args="--jinja"` flag is required for Ramalama to work with the goose Ollama provider.

          Example:

          ```sh
          ramalama serve --runtime-args="--jinja" ollama://qwen2.5
          ```

          3. In a separate terminal window, configure with goose:

          ```sh
          goose configure
          ```

          4. Choose to `Configure Providers`

          ```
          ┌   goose-configure
          │
          ◆  What would you like to configure?
          │  ● Configure Providers (Change provider or update credentials)
          │  ○ Toggle Extensions
          │  ○ Add Extension
          └
          ```

          5. Choose `Ollama` as the model provider since Ramalama is API compatible and can use the goose Ollama provider

          ```
          ┌   goose-configure
          │
          ◇  What would you like to configure?
          │  Configure Providers
          │
          ◆  Which model provider should we use?
          │  ○ Anthropic
          │  ○ Databricks
          │  ○ Google Gemini
          │  ○ Groq
          │  ● Ollama (Local open source models)
          │  ○ OpenAI
          │  ○ OpenRouter
          └
          ```

          6. Enter the host where your model is running

          :::info Endpoint
          For the Ollama provider, if you don't provide a host, we set it to `localhost:11434`. When constructing the URL, we prepend `http://` if the scheme is not `http` or `https`. Since Ramalama's default port to serve on is 8080, we set `OLLAMA_HOST=http://0.0.0.0:8080`
          :::

          ```
          ┌   goose-configure
          │
          ◇  What would you like to configure?
          │  Configure Providers
          │
          ◇  Which model provider should we use?
          │  Ollama
          │
          ◆  Provider Ollama requires OLLAMA_HOST, please enter a value
          │  http://0.0.0.0:8080
          └
          ```


          7. Enter the model you have running

          ```
          ┌   goose-configure
          │
          ◇  What would you like to configure?
          │  Configure Providers
          │
          ◇  Which model provider should we use?
          │  Ollama
          │
          ◇  Provider Ollama requires OLLAMA_HOST, please enter a value
          │  http://0.0.0.0:8080
          │
          ◇  Enter a model from that provider:
          │  qwen2.5
          │
          ◇  Welcome! You're all set to explore and utilize my capabilities. Let's get started on solving your problems together!
          │
          └  Configuration saved successfully
          ```

          :::tip Context Length
          If you notice that goose is having trouble using extensions or is ignoring [.goosehints](/docs/guides/context-engineering/using-goosehints), it is likely that the model's default context length of 2048 tokens is too low. Use `ramalama serve` to set the `--ctx-size, -c` option to a [higher value](https://github.com/containers/ramalama/blob/main/docs/ramalama-serve.1.md#--ctx-size--c).
          :::

      </TabItem>
      <TabItem value="deepseek" label="DeepSeek-R1">
        The native `DeepSeek-r1` model doesn't support tool calling, however, we have a [custom model](https://ollama.com/michaelneale/deepseek-r1-goose) you can use with goose.

        :::warning
        Note that this is a 70B model size and requires a powerful device to run smoothly.
        :::


        1. [Download Ollama](https://ollama.com/download).
        2. In a terminal window, run the following command to install the custom DeepSeek-r1 model:

        ```sh
        ollama run michaelneale/deepseek-r1-goose
        ```

        3. In a separate terminal window, configure with goose:

        ```sh
        goose configure
        ```

        4. Choose to `Configure Providers`

        ```
        ┌   goose-configure
        │
        ◆  What would you like to configure?
        │  ● Configure Providers (Change provider or update credentials)
        │  ○ Toggle Extensions
        │  ○ Add Extension
        └
        ```

        5. Choose `Ollama` as the model provider

        ```
        ┌   goose-configure
        │
        ◇  What would you like to configure?
        │  Configure Providers
        │
        ◆  Which model provider should we use?
        │  ○ Anthropic
        │  ○ Databricks
        │  ○ Google Gemini
        │  ○ Groq
        │  ● Ollama (Local open source models)
        │  ○ OpenAI
        │  ○ OpenRouter
        └
        ```

        6. Enter the host where your model is running

        ```
        ┌   goose-configure
        │
        ◇  What would you like to configure?
        │  Configure Providers
        │
        ◇  Which model provider should we use?
        │  Ollama
        │
        ◆  Provider Ollama requires OLLAMA_HOST, please enter a value
        │  http://localhost:11434
        └
        ```

        7. Enter the installed model from above

        ```
        ┌   goose-configure
        │
        ◇  What would you like to configure?
        │  Configure Providers
        │
        ◇  Which model provider should we use?
        │  Ollama
        │
        ◇   Provider Ollama requires OLLAMA_HOST, please enter a value
        │  http://localhost:11434
        │
        ◇  Enter a model from that provider:
        │  michaelneale/deepseek-r1-goose
        │
        ◇  Welcome! You're all set to explore and utilize my capabilities. Let's get started on solving your problems together!
        │
        └  Configuration saved successfully
        ```
      </TabItem>
      <TabItem value="others" label="Other Models" default>
        1. [Download Ollama](https://ollama.com/download).
        2. In a terminal, run any [model supporting tool-calling](https://ollama.com/search?c=tools)

          Example:

          ```sh
          ollama run qwen2.5
          ```

        3. In a separate terminal window, configure with goose:

          ```sh
          goose configure
          ```

        4. Choose to `Configure Providers`

        ```
        ┌   goose-configure
        │
        ◆  What would you like to configure?
        │  ● Configure Providers (Change provider or update credentials)
        │  ○ Toggle Extensions
        │  ○ Add Extension
        └
        ```

        5. Choose `Ollama` as the model provider

        ```
        ┌   goose-configure
        │
        ◇  What would you like to configure?
        │  Configure Providers
        │
        ◆  Which model provider should we use?
        │  ○ Anthropic
        │  ○ Databricks
        │  ○ Google Gemini
        │  ○ Groq
        │  ● Ollama (Local open source models)
        │  ○ OpenAI
        │  ○ OpenRouter
        └
        ```

        6. Enter the host where your model is running

        :::info Endpoint
        For Ollama, if you don't provide a host, we set it to `localhost:11434`.
        When constructing the URL, we prepend `http://` if the scheme is not `http` or `https`.
        If you're running Ollama on a different server, you'll have to set `OLLAMA_HOST=http://{host}:{port}`.
        For hosted models on ollama.com, use the **Ollama Cloud** provider instead.
        :::

        ```
        ┌   goose-configure
        │
        ◇  What would you like to configure?
        │  Configure Providers
        │
        ◇  Which model provider should we use?
        │  Ollama
        │
        ◆  Provider Ollama requires OLLAMA_HOST, please enter a value
        │  http://localhost:11434
        └
        ```


        7. Enter the model you have running

        ```
        ┌   goose-configure
        │
        ◇  What would you like to configure?
        │  Configure Providers
        │
        ◇  Which model provider should we use?
        │  Ollama
        │
        ◇  Provider Ollama requires OLLAMA_HOST, please enter a value
        │  http://localhost:11434
        │
        ◇  Enter a model from that provider:
        │  qwen2.5
        │
        ◇  Welcome! You're all set to explore and utilize my capabilities. Let's get started on solving your problems together!
        │
        └  Configuration saved successfully
        ```

        :::tip Context Length
        If you notice that goose is having trouble using extensions or is ignoring [.goosehints](/docs/guides/context-engineering/using-goosehints), it is likely that the model's default context length of 4096 tokens is too low. Set the `OLLAMA_CONTEXT_LENGTH` environment variable to a [higher value](https://github.com/ollama/ollama/blob/main/docs/faq.mdx#how-can-i-specify-the-context-window-size).
        :::

      </TabItem>
    </Tabs>
  </TabItem>
  <TabItem value="lmstudio" label="LM Studio">
    [LM Studio](https://lmstudio.ai/) lets you run open-source models locally with an OpenAI-compatible API server.

    1. Download and install LM Studio.
    2. Open LM Studio and download a model that supports tool calling (e.g., Qwen, Llama, or Mistral variants).
    3. Start the local server in LM Studio. The server runs on `http://localhost:1234` by default

    4. Configure goose to use LM Studio:

    <Tabs groupId="interface">
      <TabItem value="ui" label="goose Desktop" default>
        1. Click the <PanelLeft className="inline" size={16} /> button in the top-left to open the sidebar.
        2. Click the `Settings` button on the sidebar.
        3. Click the `Models` tab.
        4. Click `Configure providers`.
        5. Choose `LM Studio` from the provider list and click `Configure`.
        6. Click `Submit` (no API key is needed).
        7. Select the model you have loaded in LM Studio.
      </TabItem>
      <TabItem value="cli" label="goose CLI">
        1. Run:
        ```sh
        goose configure
        ```
        2. Select `Configure Providers` from the menu.
        3. Choose `LM Studio` as the provider.
        4. Enter the model name that matches the model loaded in LM Studio.

        ```
        ┌   goose-configure
        │
        ◇  What would you like to configure?
        │  Configure Providers
        │
        ◇  Which model provider should we use?
        │  LM Studio
        │
        ◇  Enter a model from that provider:
        │  qwen2.5-7b-instruct
        │
        └  Configuration saved successfully
        ```
      </TabItem>
    </Tabs>

    :::tip Model Name
    Make sure the model name you enter in goose matches the model identifier shown in LM Studio's server panel.
    :::
  </TabItem>
  <TabItem value="atomic-chat" label="Atomic Chat">
    [Atomic Chat](https://github.com/AtomicBot-ai/Atomic-Chat) lets you run open-source models locally with an OpenAI-compatible API server.

    1. Download and install Atomic Chat from [atomic.chat](https://atomic.chat/) or [GitHub Releases](https://github.com/AtomicBot-ai/Atomic-Chat/releases).
    2. Open Atomic Chat and download a model that supports tool calling (e.g., Qwen, Llama, or Mistral variants).
    3. Start the local server in Atomic Chat. The server runs on `http://localhost:1337` by default

    4. Configure goose to use Atomic Chat:

    <Tabs groupId="interface">
      <TabItem value="ui" label="goose Desktop" default>
        1. Click the <PanelLeft className="inline" size={16} /> button in the top-left to open the sidebar.
        2. Click the `Settings` button on the sidebar.
        3. Click the `Models` tab.
        4. Click `Configure providers`.
        5. Choose `Atomic Chat` from the provider list and click `Configure`.
        6. Click `Submit` (no API key is needed).
        7. Select the model you have loaded in Atomic Chat.
      </TabItem>
      <TabItem value="cli" label="goose CLI">
        1. Run:
        ```sh
        goose configure
        ```
        2. Select `Configure Providers` from the menu.
        3. Choose `Atomic Chat` as the provider.
        4. Enter the model name that matches the model loaded in Atomic Chat.

        ```
        ┌   goose-configure
        │
        ◇  What would you like to configure?
        │  Configure Providers
        │
        ◇  Which model provider should we use?
        │  Atomic Chat
        │
        ◇  Enter a model from that provider:
        │  qwen2.5-7b-instruct
        │
        └  Configuration saved successfully
        ```
      </TabItem>
    </Tabs>

    :::tip Model Name
    Make sure the model name you enter in goose matches the model identifier shown for your server in Atomic Chat. If the API listens on a different origin than `http://localhost:1337`, set `ATOMIC_CHAT_HOST` in goose to match (scheme, host, and port only).
    :::
  </TabItem>
  <TabItem value="docker" label="Docker Model Runner" default>
    1. [Get Docker](https://docs.docker.com/get-started/get-docker/)
    2. [Enable Docker Model Runner](https://docs.docker.com/ai/model-runner/#enable-dmr-in-docker-desktop)
    3. [Pull a model](https://docs.docker.com/ai/model-runner/#pull-a-model), for example, from Docker Hub [AI namespace](https://hub.docker.com/u/ai), [Unsloth](https://hub.docker.com/u/unsloth), or [from HuggingFace](https://www.docker.com/blog/docker-model-runner-on-hugging-face/)

    Example:

    ```sh
    docker model pull hf.co/unsloth/gemma-3n-e4b-it-gguf:q6_k
    ```

    4. Configure goose to use Docker Model Runner, using the OpenAI API compatible endpoint:

    ```sh
    goose configure
    ```

    5. Choose to `Configure Providers`

    ```
    ┌   goose-configure
    │
    ◆  What would you like to configure?
    │  ● Configure Providers (Change provider or update credentials)
    │  ○ Toggle Extensions
    │  ○ Add Extension
    └
    ```

    6. Choose `OpenAI` as the model provider:

    ```
    ┌   goose-configure
    │
    ◇  What would you like to configure?
    │  Configure Providers
    │
    ◆  Which model provider should we use?
    │  ○ Anthropic
    │  ○ Amazon Bedrock
    │  ○ Claude Code
    │  ● OpenAI (GPT-4 and other OpenAI models, including OpenAI compatible ones)
    │  ○ OpenRouter
    ```

    7. Configure Docker Model Runner endpoint as the `OPENAI_HOST`:

    ```
    ┌   goose-configure
    │
    ◇  What would you like to configure?
    │  Configure Providers
    │
    ◇  Which model provider should we use?
    │  OpenAI
    │
    ◆  Provider OpenAI requires OPENAI_HOST, please enter a value
    │  https://api.openai.com (default)
    └
    ```

    The default value for the host-side port Docker Model Runner is 12434, so the `OPENAI_HOST` value could be:
    `http://localhost:12434`.

    8. Configure the base path:

    ```
    ◆  Provider OpenAI requires OPENAI_BASE_PATH, please enter a value
    │  v1/chat/completions (default)
    └
    ```

    Docker model runner uses `/engines/llama.cpp/v1/chat/completions` for the base path.

    9. Finally configure the model available in Docker Model Runner to be used by goose: `hf.co/unsloth/gemma-3n-e4b-it-gguf:q6_k`

    ```
    │
    ◇  Enter a model from that provider:
    │  gpt-4o
    │
    ◒  Checking your configuration...
    └  Configuration saved successfully
    ```
  </TabItem>
</Tabs>



## OpenRouter Advanced Parameters

OpenRouter accepts provider-specific request parameters such as `verbosity`, `reasoning`, `plugins`, `require_parameters`, and other supported fields. Set `OPENROUTER_PARAMETERS` in your `config.yaml` to add these fields to every OpenRouter chat completion request.

You can use a YAML object:

```yaml
OPENROUTER_PARAMETERS:
  verbosity: xhigh
  reasoning:
    effort: high
  plugins:
    - id: web
```

Or a JSON string:

```yaml
OPENROUTER_PARAMETERS: '{"verbosity":"xhigh","plugins":[{"id":"web"}]}'
```

goose ignores reserved request fields it already manages, such as `model`, `messages`, `stream`, and `stream_options`. Other OpenRouter-specific top-level fields are passed through the shared OpenAI-compatible request parameter handling.

## GitHub Copilot Authentication

GitHub Copilot uses a device flow for authentication, so no API keys are required:

1. Run [`goose configure`](#configure-provider-and-model) and select **GitHub Copilot**
2. An eight-character code will be automatically copied to your clipboard
3. A browser will open to GitHub's device activation page
4. Paste the code to authorize the application
5. When you return to goose, GitHub Copilot will be available as a provider in both CLI and Desktop.

## Azure OpenAI Authentication

goose supports three authentication methods for Azure OpenAI:

1. **Entra ID Bearer Token** - Uses a pre-acquired Microsoft Entra access token from `AZURE_OPENAI_AD_TOKEN`, sent as `Authorization: Bearer <token>`. goose skips Azure CLI and token acquisition entirely, which suits enterprise deployments where only short-lived tokens are exposed to the runtime (e.g. obtained via `az account get-access-token --resource https://cognitiveservices.azure.com --query accessToken --output tsv`)
2. **API Key Authentication** - Uses the `AZURE_OPENAI_API_KEY` for direct authentication
3. **Azure Credential Chain** - Uses Azure CLI credentials automatically without requiring an API key

When more than one is configured, `AZURE_OPENAI_AD_TOKEN` takes precedence over `AZURE_OPENAI_API_KEY`, which takes precedence over the credential chain.

To use the Azure Credential Chain:
- Ensure you're logged in with `az login`
- Have appropriate Azure role assignments for the Azure OpenAI service
- Configure with `goose configure` and select Azure OpenAI, leaving the API key field empty

This method simplifies authentication and enhances security for enterprise environments.

## Multi-Model Configuration

Beyond single-model setups, goose supports [multi-model configurations](/docs/guides/multi-model/) that can use different models and providers for specialized tasks:

- **Planning Mode** - Use a dedicated planner model to create detailed project breakdowns before execution
- **Subagents** - Delegate scoped tasks to isolated sessions to keep your primary workflow focused and efficient

## Meta Muse Spark Reasoning Effort

Meta's Muse Spark models support a configurable reasoning effort that maps to Meta's `reasoning_effort` request parameter:
- **Low** - Faster responses, lighter reasoning
- **Medium** - Balanced reasoning depth and latency
- **High** - Deeper reasoning, higher latency
- **Max** - Sent as `xhigh`, the deepest reasoning level Meta supports

<Tabs groupId="interface">
  <TabItem value="ui" label="goose Desktop" default>
    When selecting a Muse Spark model, a "Thinking Effort" dropdown appears automatically. Select your preference and the setting persists across sessions.
  </TabItem>

  <TabItem value="cli" label="goose CLI">
    When you run `goose configure` and select a Muse Spark model, you'll be prompted to choose a thinking effort:

    ```
    ◆  Select thinking effort:
    │  ● Off - No extended thinking
    │  ○ Low - Better latency, lighter reasoning
    │  ○ Medium - Moderate thinking
    │  ○ High - Deep reasoning
    │  ○ Max - No constraints on thinking depth
    ```

    You can also set this globally with the `GOOSE_THINKING_EFFORT` environment variable (`off`, `low`, `medium`, `high`, or `max`).
  </TabItem>
</Tabs>

:::note
Muse Spark always reasons and has no way to disable it, so choosing `off` is clamped to `low` (the lightest level Meta supports) rather than omitting the `reasoning_effort` parameter.
:::

## Gemini 3 Thinking Levels

Gemini 3 models support configurable thinking levels to balance response latency and reasoning depth:
- **Low** (default) - Faster responses, lighter reasoning
- **High** - Deeper reasoning, higher latency

:::tip
When thinking is enabled, you can view the model's reasoning process. See [Viewing Model Reasoning](#viewing-model-reasoning) for details.
:::

<Tabs groupId="interface">
  <TabItem value="ui" label="goose Desktop" default>
    When selecting a Gemini 3 model, a "Thinking Level" dropdown appears automatically. Select your preference and the setting persists across sessions.
  </TabItem>

  <TabItem value="cli" label="goose CLI">
    **Interactive configuration:**

    When you run `goose configure` and select a Gemini 3 model, you'll be prompted to choose a thinking level:

    ```
    ◆  Select thinking level for Gemini 3:
    │  ● Low - Better latency, lighter reasoning
    │  ○ High - Deeper reasoning, higher latency
    ```
  </TabItem>
</Tabs>

:::info Priority Order
The thinking level is determined in this order (highest to lowest priority):
1. `request_params.thinking_level` in model configuration
2. `GEMINI3_THINKING_LEVEL` environment variable
3. Default value: `low`
:::

## Viewing Model Reasoning

Some models expose their internal reasoning or "chain of thought" as part of their response. goose automatically captures this reasoning output and makes it available to you. The following models and providers support reasoning output:

| Provider / Model | How It Works |
|---|---|
| **DeepSeek-R1** (via OpenAI, Ollama, OpenRouter, OVHcloud, etc.) | Reasoning captured from the `reasoning_content` field in the API response |
| **Kimi** (via Groq or other OpenAI-compatible endpoints) | Reasoning captured from the `reasoning_content` field in the API response |
| **Gemini CLI** (Google Gemini models with thinking enabled) | Thinking blocks captured from the streaming response |
| **Claude** (Anthropic, with [Claude thinking](/docs/guides/environment-variables#claude-thinking-configuration) enabled) | Thinking blocks captured from the API response |

<Tabs groupId="interface">
  <TabItem value="ui" label="goose Desktop" default>
    Reasoning output appears automatically in a collapsible **"Show reasoning"** toggle above the model's response. Click it to expand and view the model's thought process.
  </TabItem>

  <TabItem value="cli" label="goose CLI">
    Reasoning output is **hidden by default** in the CLI. To display it, set the `GOOSE_CLI_SHOW_THINKING` environment variable:

    ```bash
    export GOOSE_CLI_SHOW_THINKING=1
    ```

    When enabled, reasoning appears under a "Thinking:" header in dimmed text before the model's main response.

    :::note
    This requires stdout to be a terminal (reasoning output won't appear when piping output to a file or another command).
    :::
  </TabItem>
</Tabs>

:::tip
Reasoning output can be useful for understanding how the model arrived at its answer, debugging unexpected behavior, or learning from the model's problem-solving approach. However, it can also be verbose — toggle it on only when you need it.
:::

---

If you have any questions or need help with a specific provider, feel free to reach out to us on [Discord](https://discord.gg/n8R5VaWDAn) or on the [goose repo](https://github.com/aaif-goose/goose).


[providers]: /docs/getting-started/providers
[function-calling-leaderboard]: https://gorilla.cs.berkeley.edu/leaderboard.html
