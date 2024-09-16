from typing import List
from unittest.mock import MagicMock

import pytest
from exchange import Exchange
from exchange.message import Message
from exchange.providers import Provider, Usage
from exchange.tool import Tool
from exchange.moderators import PassiveModerator
from goose.toolkit.pseudotool import decode, pseudotool


def greet(who: str, n: int):
    """Send greeting

    Args:
        who (str): Who
        n (int): ignored
    """
    return n * who


class MockProvider(Provider):
    def __init__(self, sequence: List[Message], usage_dicts: List[dict]):
        # We'll use init to provide a preplanned reply sequence
        self.sequence = sequence
        self.call_count = 0
        self.usage_dicts = usage_dicts

    @staticmethod
    def get_usage(data: dict) -> Usage:
        usage = data.pop("usage")
        input_tokens = usage.get("input_tokens")
        output_tokens = usage.get("output_tokens")
        total_tokens = usage.get("total_tokens")

        if (
            total_tokens is None
            and input_tokens is not None
            and output_tokens is not None
        ):
            total_tokens = input_tokens + output_tokens

        return Usage(
            input_tokens=input_tokens,
            output_tokens=output_tokens,
            total_tokens=total_tokens,
        )

    def complete(
        self, model: str, system: str, messages: List[Message], tools: List[Tool]
    ) -> Message:
        output = self.sequence[self.call_count]
        usage = self.get_usage(self.usage_dicts[self.call_count])
        self.call_count += 1
        return (output, usage)


@pytest.fixture
def tool():
    return Tool.from_function(greet)


def test_decode(tool):
    valid = """
```json
{"who": "world", "n": 3}
```
"""
    assert decode(valid, tool.parameters) == {"who": "world", "n": 3}

    invalid_block = """anything"""
    with pytest.raises(ValueError, match="code block"):
        decode(invalid_block, tool.parameters)

    invalid_json = (
        valid
    ) = """
```json
{"who": "world", "n": 3
```
"""
    with pytest.raises(ValueError, match="json"):
        decode(invalid_json, tool.parameters)

    missing_who = (
        valid
    ) = """
```json
{"n": 3}
```
"""
    with pytest.raises(ValueError, match="who"):
        decode(missing_who, tool.parameters)

    extra_field = (
        valid
    ) = """
```json
{"who": "world", "n": 3, "invalid": 4}
```
"""
    with pytest.raises(ValueError, match="invalid"):
        decode(extra_field, tool.parameters)


def test_pseudotool(tool):
    valid = """
```json
{"who": "world", "n": 3}
```
"""
    mockprovider = MockProvider(
        sequence=[Message.assistant(valid)],
        usage_dicts=[{"usage": {"input_tokens": 12, "output_tokens": 23}}],
    )
    exchange = Exchange(
        provider=mockprovider,
        model="na",
        system="na",
        moderator=PassiveModerator(),
    )
    exchange.add(Message.user("please send 3 greetings to world"))
    # tests that everything is working
    response = pseudotool(exchange, tool)
    assert response == "worldworldworld"
