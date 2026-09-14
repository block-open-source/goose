# goose-provider-types

The provider contract and the conversation types that flow through it. This is the
crate to depend on if you want to implement a provider, or to work with goose
messages without pulling in the whole agent.

Provider implementations live in [`goose-providers`](../goose-providers), which
re-exports every module here.

## The `Provider` trait

```rust
#[async_trait]
pub trait Provider: Send + Sync {
    fn get_name(&self) -> &str;

    async fn stream(
        &self,
        model_config: &ModelConfig,
        system: &str,
        messages: &[Message],
        tools: &[Tool],
    ) -> Result<MessageStream, ProviderError>;
}
```

`stream` is the only required method. Everything else has a default:

- `complete` collects the stream into a single `(Message, ProviderUsage)`.
- `get_context_limit` falls back to the model config's limit.
- `fetch_supported_models` / `fetch_supported_model_info` / `fetch_model_info`
  describe the provider's inventory; `fetch_recommended_models` filters it
  through the bundled canonical registry (and keeps tool-less models when
  toolshim emulation is on).
- `retry_config`, `resume`, and `provider_session_id` cover retries and
  provider-side session state.

A `MessageStream` yields partial text but *complete* tool calls — a text chunk may
be a single word, while a tool call is only emitted once fully assembled. Helpers:
`collect_stream` and `stream_from_single_message`.

Static metadata is declared separately through `ProviderDescriptor::metadata()`,
returning `ProviderMetadata` with its `ConfigKey`s, models, and any
`ProviderDeprecation`.

## Modules

| Module | Contents |
| --- | --- |
| `base` | `Provider`, `ProviderDescriptor`, `ProviderMetadata`, `ModelInfo`, `ConfigKey`, `MessageStream`, `PermissionRouting` |
| `conversation` | `Conversation`, `message`, `token_usage`, `tool_request` — the message model shared across the workspace |
| `model` | `ModelConfig`: model name, context limit, reasoning detection |
| `canonical` | Bundled canonical model registry and provider-model mapping |
| `errors` | `ProviderError` |
| `formats`, `json`, `images`, `mcp_utils` | Wire-format conversion helpers |
| `cache_semantics`, `thinking` | Prompt caching and reasoning/thinking-block handling |
| `retry` | `RetryConfig` |
| `permission`, `goose_mode` | Tool-approval modes |
| `request_log`, `utils` | Request logging and shared helpers |
