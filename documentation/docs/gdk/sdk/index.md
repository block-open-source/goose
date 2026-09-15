---
sidebar_position: 1
title: SDK
sidebar_label: Overview
description: Build with goose providers in Rust, Python, and Kotlin.
---

# SDK

The SDK exposes its provider layer as an in-process library so you can
call models, stream completions, and compact conversations from your own
application.

One Rust crate, `goose-sdk`, is the source of every language binding. Python and
Kotlin are generated from it with [UniFFI](https://github.com/mozilla/uniffi-rs),
so all three languages share the same types, behavior, and version number.

See the [API Reference](/docs/gdk/sdk/api-reference) for the complete
surface in your language of choice.

:::info Alpha
The SDK is in alpha. The surface may change between `0.x` releases. Pin an
exact version and check the API reference version selector when upgrading.
:::

## What you can do

- Construct providers for OpenAI, Anthropic, Groq, Databricks, or any
  [declarative provider](#declarative-providers) defined in JSON
- Stream a completion chunk by chunk, including tool calls and reasoning output
- Request a single non-streaming completion
- Compact a long conversation into a summary so it can continue past the
  model's context window
- Capture provider request logs as JSONL

## Install

<!-- prettier-ignore-start -->

### Rust

```bash
cargo add goose-sdk --features uniffi
```

The `uniffi` feature enables the in-process provider API documented here.

### Python

```bash
pip install goose-sdk
```

The package installs as `goose-sdk` and imports as `goose`. Wheels bundle the
native library, so there is nothing else to build. Requires Python 3.9+.

```python
import goose
```

### Kotlin / JVM

```kotlin
dependencies {
    implementation("io.github.aaif-goose:gdk:<version>")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-core:1.10.2")
}
```

The artifact version matches the Rust crate version. Classes live in the
`io.github.aaif_goose` package. The jar bundles native libraries for
macOS (arm64, x86-64), Linux (arm64, x86-64), and Windows (x86-64).

On JDK 24+, add `--enable-native-access=ALL-UNNAMED` because the GDK loads its
native library through JNA.

<!-- prettier-ignore-end -->

## Quickstart

Each example builds a provider, sends one message, and prints the streamed
response.

### Python

```python
import asyncio
from goose import (
    MessageContent,
    MessageRole,
    ProviderMessage,
    ProviderModelConfig,
    StreamChunk,
    openai_default_model,
    openai_provider,
)


async def main() -> None:
    provider = openai_provider(api_key="...")
    model = ProviderModelConfig(model_name=openai_default_model())
    messages = [
        ProviderMessage(
            role=MessageRole.USER,
            content=[MessageContent.Text(text="What is the capital of France?")],
        )
    ]

    stream = await provider.stream(model, "You are a geography expert.", messages, [])
    while chunk := await stream.next_chunk():
        if isinstance(chunk, StreamChunk.TextChunk):
            print(chunk.text, end="")


asyncio.run(main())
```

### Kotlin

```kotlin
import io.github.aaif_goose.MessageContent
import io.github.aaif_goose.MessageRole
import io.github.aaif_goose.ProviderMessage
import io.github.aaif_goose.ProviderModelConfig
import io.github.aaif_goose.StreamChunk
import io.github.aaif_goose.streamFlow
import io.github.aaif_goose.providers.openai.defaultModel
import io.github.aaif_goose.providers.openai.provider as openAiProvider
import kotlinx.coroutines.runBlocking

fun main() = runBlocking {
    val provider = openAiProvider(System.getenv("OPENAI_API_KEY"))
    val model = ProviderModelConfig(modelName = defaultModel())
    val messages = listOf(
        ProviderMessage(
            role = MessageRole.USER,
            content = listOf(MessageContent.Text(text = "What is the capital of France?")),
        ),
    )

    provider.streamFlow(model, "You are a geography expert.", messages)
        .collect { chunk ->
            if (chunk is StreamChunk.TextChunk) print(chunk.text)
        }
}
```

### Rust

```rust
use goose_sdk::bindings::{
    openai_default_model, openai_provider, MessageContent, MessageRole, ProviderMessage,
    ProviderModelConfig, StreamChunk,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let provider = openai_provider(std::env::var("OPENAI_API_KEY")?)?;
    let model = ProviderModelConfig {
        model_name: openai_default_model(),
        ..Default::default()
    };
    let messages = vec![ProviderMessage {
        role: MessageRole::User,
        content: vec![MessageContent::Text {
            text: "What is the capital of France?".to_string(),
        }],
    }];

    let stream = provider
        .stream(model, "You are a geography expert.".to_string(), messages, vec![])
        .await?;

    while let Some(chunk) = stream.next_chunk().await? {
        if let StreamChunk::TextChunk { text } = chunk {
            print!("{text}");
        }
    }
    Ok(())
}
```

## Kotlin idioms

The Kotlin package adds a few conveniences on top of the generated bindings:

| Kotlin API | Equivalent generated call |
| --- | --- |
| `provider.streamFlow(model, system, messages, tools)` | `stream(...)` plus a `nextChunk()` loop, as a `Flow<StreamChunk>` |
| `providers.openai.provider(apiKey)` | `openaiProvider(apiKey)` |
| `providers.openai.defaultModel()` | `openaiDefaultModel()` |
| `providers.anthropic.provider(apiKey, baseUrl, betaHeaders)` | `anthropicProvider(...)` |
| `providers.groq.provider(apiKey)` | `groqProvider(apiKey)` |
| `providers.databricks.provider(host, token)` | `databricksProvider(host, token)` |

`tools` defaults to an empty list in the Kotlin helpers, and suspending
functions map to Kotlin coroutines. Errors surface as `GooseException`
subclasses.

## Declarative providers

Any provider that speaks an OpenAI- or Anthropic-compatible API can be defined
in JSON and loaded without new Rust code:

```python
provider = goose.declarative_provider_from_json(open("deepseek.json").read())
```

Environment variable placeholders such as `${DEEPSEEK_API_KEY}` in the JSON are
resolved when the provider is constructed.

## Streaming model

`stream()` returns a `ProviderStream`. Call `next_chunk()` until it returns
`None` to consume the response:

| Chunk | Meaning |
| --- | --- |
| `TextChunk` | Assistant text |
| `ToolChunk` | A tool call request with JSON arguments and the provider's tool-call `index` |
| `ThinkingChunk` / `RedactedThinkingChunk` | Reasoning output |
| `EndChunk` | Stream finished, carries final token `Usage` |
| `ErrorChunk` | Mid-stream failure, carries a `GooseStreamError` |

Errors raised before the stream starts are thrown as `GooseError`
(`GooseException` in Kotlin). Errors that occur mid-stream arrive as an
`ErrorChunk` instead.
