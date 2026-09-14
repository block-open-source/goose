# goose-context-management

Conversation compaction: summarizing a message history down to a single message so
a conversation can continue past a model's context window.

The crate is layered, smallest first — take the layer you need.

## `summarize`

Given a model and a slice of messages, produce one summary message.

```rust
use goose_context_management::{summarize, Templates};

let summary = summarize(&model, None, &Templates::default(), &messages).await?;
// summary.message, summary.usage
```

## `compact`

The trait-based API, for callers that own their own conversation representation.
Implement `CompactionInput` to expose messages (and optionally `Templates`), and
`CompactionOutput` to receive the summary and usage:

```rust
pub trait CompactionInput {
    fn messages(&self) -> Vec<Message>;
    fn templates(&self) -> Templates { Templates::default() }
}

pub trait CompactionOutput {
    fn set_summary(&mut self, summary: Message);
    fn set_usage(&mut self, usage: ProviderUsage);
}
```

`Vec<Message>` already implements `CompactionInput`, so the simple case needs no
wrapper type.

## Other exports

- `CompactionModel` — the model abstraction compaction runs against, with
  `ProviderModel` adapting any [`goose-providers`](../goose-providers) provider.
- `TokenEstimator` — optional token counting, used to decide how much history to
  feed the summarizer.
- `CompactingProvider` — a `Provider` wrapper that compacts as it goes.
- `StructuredSummary` / `FileActivity` — structured summary output, including
  which files the conversation touched.
- `Templates` and `format_message_for_compacting` — prompt shaping.
- `DEFAULT_COMPACTION_THRESHOLD` (`0.8`) — the default fraction of the context
  window at which callers compact.

## Cross-language access

Python and Kotlin reach compaction through [`goose-sdk`](../goose-sdk), which
wraps this crate in its UniFFI bindings. The trait-based `compact` API is Rust
only.
