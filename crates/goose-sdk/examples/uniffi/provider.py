#!/usr/bin/env -S uv run --script
"""GDK demo: build a declarative provider and stream a completion."""
import asyncio
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent.parent / "generated"))

from goose import (  # noqa: E402
    MessageContent,
    MessageRole,
    ProviderMessage,
    ProviderModelConfig,
    StreamChunk,
    declarative_provider_from_json,
)


async def main() -> None:
    provider = declarative_provider_from_json((HERE.parent / "deepseek.json").read_text())
    model = ProviderModelConfig(model_name="deepseek-v4-flash")
    messages = [
        ProviderMessage(
            role=MessageRole.USER,
            content=[MessageContent.Text(text="what is the capital of France?")],
        )
    ]
    stream = await provider.stream(
        model,
        "You are a knowledgable geography expert",
        messages,
        [],
    )

    while chunk := await stream.next_chunk():
        if isinstance(chunk, StreamChunk.TextChunk):
            print(chunk.text, end="")
        elif isinstance(chunk, StreamChunk.EndChunk) and chunk.usage:
            print(f"\nusage: {chunk.usage}")
        elif isinstance(chunk, StreamChunk.ErrorChunk):
            print(f"\nerror: {chunk.error.message}", file=sys.stderr)
    print()


if __name__ == "__main__":
    asyncio.run(main())
