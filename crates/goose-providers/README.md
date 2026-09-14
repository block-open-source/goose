# goose-providers

Provider implementations for goose. The trait they implement and the conversation
types they exchange live in [`goose-provider-types`](../goose-provider-types),
which this crate re-exports — depend on this crate when you want working
providers, and on the types crate when you only need the contract.

## Native providers

| Module | Provider |
| --- | --- |
| `anthropic` | Anthropic |
| `openai` | OpenAI |
| `openai_compatible` | Any OpenAI-compatible endpoint |
| `google` | Google Gemini |
| `databricks`, `databricks_v2`, `databricks_auth` | Databricks, including OAuth |
| `azure_foundry` | Azure AI Foundry |
| `snowflake` | Snowflake Cortex |
| `ollama` | Ollama |
| `local_inference` | On-device models (requires `local-inference`) |

## Declarative providers

Most OpenAI-compatible services don't need Rust code — they're a JSON file in
`src/declarative/definitions/` (Groq, Mistral, Together, Cerebras, DeepSeek,
Perplexity, LM Studio, Vercel AI Gateway, and ~30 more). Each definition declares
its engine, base URL, env vars, and models.

`declarative` exposes the same shape at runtime:

- `deserialize_provider_config` / `from_json` — build a `DeclarativeProviderConfig`
  from JSON.
- `load_custom_providers(dir)` — load user-supplied definitions from disk.
- `fixed_provider_configs` — the bundled set.

```bash
cargo run -p goose-providers --example declarative
cargo run -p goose-providers --example streaming
```

## Features

Default is `[]`.

- **TLS (pick one):** `rustls-tls` or `native-tls`.
- `local-inference` — pulls in [`goose-local-inference`](../goose-local-inference);
  `cuda`, `vulkan`, `mlx` select an accelerator and imply it.

## Shared plumbing

`api_client` (HTTP with auth and retries), `http_status` (mapping responses to
`ProviderError`), and the re-exported `retry`, `cache_semantics`, `thinking`, and
`formats` modules are what the provider implementations are built from — start
there when adding a new one.
