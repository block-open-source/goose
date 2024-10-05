import os

import pytest
from exchange.content import ToolResult, ToolUse
from exchange.exchange import Exchange
from exchange.message import Message
from exchange.moderators import ContextTruncate
from exchange.providers import get_provider

cases = [
    (get_provider("openai"), os.getenv("OPENAI_MODEL", "gpt-4o-mini")),
]


@pytest.mark.integration  # skipped in CI/CD
@pytest.mark.parametrize("provider,model", cases)
def test_simple(provider, model):
    provider = provider.from_env()

    ex = Exchange(
        provider=provider,
        model=model,
        moderator=ContextTruncate(model),
        system="You are a helpful assistant.",
    )

    ex.add(Message.user("What does the first entry in the menu say?"))
    ex.add(
        Message(
            role="assistant",
            content=[ToolUse(id="xyz", name="screenshot", parameters={})],
        )
    )
    ex.add(
        Message(
            role="user",
            content=[ToolResult(tool_use_id="xyz", output='"image:tests/test_image.png"')],
        )
    )

    response = ex.reply()

    # It's possible this can be flakey, but in experience so far haven't seen it
    assert "ask goose" in response.text.lower()
