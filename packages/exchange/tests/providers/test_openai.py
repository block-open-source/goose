import os

import pytest

from exchange import Text, ToolUse
from exchange.providers.openai import OpenAiProvider
from .conftest import complete, vision, tools

OPENAI_MODEL = os.getenv("OPENAI_MODEL", "gpt-4o-mini")


@pytest.mark.vcr()
def test_openai_complete(default_openai_env):
    reply_message, reply_usage = complete(OpenAiProvider, OPENAI_MODEL)

    assert reply_message.content == [Text(text="Hello! How can I assist you today?")]
    assert reply_usage.total_tokens == 27


@pytest.mark.integration
def test_openai_complete_integration():
    reply = complete(OpenAiProvider, OPENAI_MODEL)

    assert reply[0].content is not None
    print("Completion content from OpenAI:", reply[0].content)


@pytest.mark.vcr()
def test_openai_tools(default_openai_env):
    reply_message, reply_usage = tools(OpenAiProvider, OPENAI_MODEL)

    tool_use = reply_message.content[0]
    assert isinstance(tool_use, ToolUse), f"Expected ToolUse, but was {type(tool_use).__name__}"
    assert tool_use.id == "call_xXYlw4A7Ud1qtCopuK5gEJrP"
    assert tool_use.name == "read_file"
    assert tool_use.parameters == {"filename": "test.txt"}
    assert reply_usage.total_tokens == 122


@pytest.mark.integration
def test_openai_tools_integration():
    reply = tools(OpenAiProvider, OPENAI_MODEL)

    tool_use = reply[0].content[0]
    assert isinstance(tool_use, ToolUse), f"Expected ToolUse, but was {type(tool_use).__name__}"
    assert tool_use.id is not None
    assert tool_use.name == "read_file"
    assert tool_use.parameters == {"filename": "test.txt"}


@pytest.mark.vcr()
def test_openai_vision(default_openai_env):
    reply_message, reply_usage = vision(OpenAiProvider, OPENAI_MODEL)

    assert reply_message.content == [Text(text='The first entry in the menu says "Ask Goose."')]
    assert reply_usage.total_tokens == 14241


@pytest.mark.integration
def test_openai_vision_integration():
    reply = vision(OpenAiProvider, OPENAI_MODEL)

    assert "ask goose" in reply[0].text.lower()
