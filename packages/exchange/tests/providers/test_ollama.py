import os

import pytest

from exchange import Text, ToolUse
from exchange.providers.ollama import OllamaProvider, OLLAMA_MODEL
from .conftest import complete, tools

OLLAMA_MODEL = os.getenv("OLLAMA_MODEL", OLLAMA_MODEL)


@pytest.mark.vcr()
def test_ollama_complete():
    reply_message, reply_usage = complete(OllamaProvider, OLLAMA_MODEL)

    assert reply_message.content == [Text(text="Hello! I'm here to help. How can I assist you today? Let's chat. 😊")]
    assert reply_usage.total_tokens == 33


@pytest.mark.integration
def test_ollama_complete_integration():
    reply = complete(OllamaProvider, OLLAMA_MODEL)

    assert reply[0].content is not None
    print("Completion content from OpenAI:", reply[0].content)


@pytest.mark.vcr()
def test_ollama_tools():
    reply_message, reply_usage = tools(OllamaProvider, OLLAMA_MODEL)

    tool_use = reply_message.content[0]
    assert isinstance(tool_use, ToolUse), f"Expected ToolUse, but was {type(tool_use).__name__}"
    assert tool_use.id == "call_z6fgu3z3"
    assert tool_use.name == "read_file"
    assert tool_use.parameters == {"filename": "test.txt"}
    assert reply_usage.total_tokens == 133


@pytest.mark.integration
def test_ollama_tools_integration():
    reply = tools(OllamaProvider, OLLAMA_MODEL)

    tool_use = reply[0].content[0]
    assert isinstance(tool_use, ToolUse), f"Expected ToolUse, but was {type(tool_use).__name__}"
    assert tool_use.id is not None
    assert tool_use.name == "read_file"
    assert tool_use.parameters == {"filename": "test.txt"}
