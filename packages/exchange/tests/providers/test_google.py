import os
from unittest.mock import patch

import pytest
from exchange import Message, Text
from exchange.content import ToolResult, ToolUse
from exchange.providers.base import MissingProviderEnvVariableError
from exchange.providers.google import GoogleProvider
from exchange.tool import Tool
from .conftest import complete, tools, vision

GOOGLE_MODEL = os.getenv("GOOGLE_MODEL", "gemini-1.5-flash")


def example_fn(param: str) -> None:
    """
    Testing function.

    Args:
        param (str): Description of param1
    """
    pass


def test_from_env_throw_error_when_missing_api_key():
    with patch.dict(os.environ, {}, clear=True):
        with pytest.raises(MissingProviderEnvVariableError) as context:
            GoogleProvider.from_env()
        assert context.value.provider == "google"
        assert context.value.env_variable == "GOOGLE_API_KEY"
        assert "Missing environment variable: GOOGLE_API_KEY for provider google" in context.value.message
        assert "https://ai.google.dev/gemini-api/docs/api-key" in context.value.message


def test_google_response_to_text_message() -> None:
    response = {"candidates": [{"content": {"parts": [{"text": "Hello from Gemini!"}], "role": "model"}}]}
    message = GoogleProvider.google_response_to_message(response)
    assert message.content[0].text == "Hello from Gemini!"


def test_google_response_to_tool_use_message() -> None:
    response = {
        "candidates": [
            {
                "content": {
                    "parts": [{"functionCall": {"name": "example_fn", "args": {"param": "value"}}}],
                    "role": "model",
                }
            }
        ]
    }

    message = GoogleProvider.google_response_to_message(response)
    assert message.content[0].name == "example_fn"
    assert message.content[0].parameters == {"param": "value"}


def test_tools_to_google_spec() -> None:
    tools = (Tool.from_function(example_fn),)
    expected_spec = {
        "functionDeclarations": [
            {
                "name": "example_fn",
                "description": "Testing function.",
                "parameters": {
                    "type": "object",
                    "properties": {"param": {"type": "string", "description": "Description of param1"}},
                    "required": ["param"],
                },
            }
        ]
    }
    result = GoogleProvider.tools_to_google_spec(tools)
    assert result == expected_spec


def test_message_text_to_google_spec() -> None:
    messages = [Message.user("Hello, Gemini")]
    expected_spec = [{"role": "user", "parts": [{"text": "Hello, Gemini"}]}]
    result = GoogleProvider.messages_to_google_spec(messages)
    assert result == expected_spec


def test_messages_to_google_spec() -> None:
    messages = [
        Message(role="user", content=[Text("Hello, Gemini")]),
        Message(
            role="assistant",
            content=[ToolUse(id="1", name="example_fn", parameters={"param": "value"})],
        ),
        Message(role="user", content=[ToolResult(tool_use_id="1", output="Result")]),
    ]
    actual_spec = GoogleProvider.messages_to_google_spec(messages)
    #  !=
    expected_spec = [
        {"role": "user", "parts": [{"text": "Hello, Gemini"}]},
        {"role": "model", "parts": [{"functionCall": {"name": "example_fn", "args": {"param": "value"}}}]},
        {"role": "user", "parts": [{"functionResponse": {"name": "1", "response": {"content": "Result"}}}]},
    ]

    assert actual_spec == expected_spec


@pytest.mark.vcr()
def test_google_complete(default_google_env):
    reply_message, reply_usage = complete(GoogleProvider, GOOGLE_MODEL)

    assert reply_message.content == [Text("Hello! 👋  What can I do for you today? 😊 \n")]
    assert reply_usage.total_tokens == 21


@pytest.mark.integration
def test_google_complete_integration():
    reply = complete(GoogleProvider, GOOGLE_MODEL)

    assert reply[0].content is not None
    print("Completion content from Google:", reply[0].content)


@pytest.mark.vcr()
def test_google_tools(default_google_env):
    reply_message, reply_usage = tools(GoogleProvider, GOOGLE_MODEL)

    tool_use = reply_message.content[0]
    assert isinstance(tool_use, ToolUse), f"Expected ToolUse, but was {type(tool_use).__name__}"
    assert tool_use.id == "read_file"
    assert tool_use.name == "read_file"
    assert tool_use.parameters == {"filename": "test.txt"}
    assert reply_usage.total_tokens == 118


@pytest.mark.integration
def test_google_tools_integration():
    reply = tools(GoogleProvider, GOOGLE_MODEL)

    tool_use = reply[0].content[0]
    assert isinstance(tool_use, ToolUse), f"Expected ToolUse, but was {type(tool_use).__name__}"
    assert tool_use.id is not None
    assert tool_use.name == "read_file"
    assert tool_use.parameters == {"filename": "test.txt"}


@pytest.mark.vcr()
def test_google_vision(default_google_env):
    reply_message, reply_usage = vision(GoogleProvider, GOOGLE_MODEL)

    assert reply_message.content == [Text(text='The first entry in the menu says "Ask Goose 🦆".')]
    assert reply_usage.total_tokens == 298


@pytest.mark.integration
def test_google_vision_integration():
    reply = vision(GoogleProvider, GOOGLE_MODEL)

    assert "ask goose" in reply[0].text.lower()
