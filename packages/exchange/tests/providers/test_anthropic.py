import os
from unittest.mock import patch

import httpx
import pytest
from exchange import Message, Text
from exchange.content import ToolResult, ToolUse
from exchange.providers.anthropic import AnthropicProvider
from exchange.providers.base import MissingProviderEnvVariableError
from exchange.tool import Tool


def example_fn(param: str) -> None:
    """
    Testing function.

    Args:
        param (str): Description of param1
    """
    pass


@pytest.fixture
@patch.dict(os.environ, {"ANTHROPIC_API_KEY": "test_api_key"})
def anthropic_provider():
    return AnthropicProvider.from_env()


def test_from_env_throw_error_when_missing_api_key():
    with patch.dict(os.environ, {}, clear=True):
        with pytest.raises(MissingProviderEnvVariableError) as context:
            AnthropicProvider.from_env()
        assert context.value.provider == "anthropic"
        assert context.value.env_variable == "ANTHROPIC_API_KEY"
        assert context.value.message == "Missing environment variable: ANTHROPIC_API_KEY for provider anthropic."


def test_anthropic_response_to_text_message() -> None:
    response = {
        "content": [{"type": "text", "text": "Hello from Claude!"}],
    }
    message = AnthropicProvider.anthropic_response_to_message(response)
    assert message.content[0].text == "Hello from Claude!"


def test_anthropic_response_to_tool_use_message() -> None:
    response = {
        "content": [
            {
                "type": "tool_use",
                "id": "1",
                "name": "example_fn",
                "input": {"param": "value"},
            }
        ],
    }
    message = AnthropicProvider.anthropic_response_to_message(response)
    assert message.content[0].id == "1"
    assert message.content[0].name == "example_fn"
    assert message.content[0].parameters == {"param": "value"}


def test_tools_to_anthropic_spec() -> None:
    tools = (Tool.from_function(example_fn),)
    expected_spec = [
        {
            "name": "example_fn",
            "description": "Testing function.",
            "input_schema": {
                "type": "object",
                "properties": {"param": {"type": "string", "description": "Description of param1"}},
                "required": ["param"],
            },
        }
    ]
    result = AnthropicProvider.tools_to_anthropic_spec(tools)
    assert result == expected_spec


def test_message_text_to_anthropic_spec() -> None:
    messages = [Message.user("Hello, Claude")]
    expected_spec = [
        {
            "role": "user",
            "content": [{"type": "text", "text": "Hello, Claude"}],
        }
    ]
    result = AnthropicProvider.messages_to_anthropic_spec(messages)
    assert result == expected_spec


def test_messages_to_anthropic_spec() -> None:
    messages = [
        Message(role="user", content=[Text(text="Hello, Claude")]),
        Message(
            role="assistant",
            content=[ToolUse(id="1", name="example_fn", parameters={"param": "value"})],
        ),
        Message(role="user", content=[ToolResult(tool_use_id="1", output="Result")]),
    ]
    actual_spec = AnthropicProvider.messages_to_anthropic_spec(messages)
    #  !=
    expected_spec = [
        {"role": "user", "content": [{"type": "text", "text": "Hello, Claude"}]},
        {
            "role": "assistant",
            "content": [
                {
                    "type": "tool_use",
                    "id": "1",
                    "name": "example_fn",
                    "input": {"param": "value"},
                }
            ],
        },
        {
            "role": "user",
            "content": [{"type": "tool_result", "tool_use_id": "1", "content": "Result"}],
        },
    ]
    assert actual_spec == expected_spec


@patch("httpx.Client.post")
@patch("logging.warning")
@patch("logging.error")
def test_anthropic_completion(mock_error, mock_warning, mock_post, anthropic_provider):
    mock_response = {
        "content": [{"type": "text", "text": "Hello from Claude!"}],
        "usage": {"input_tokens": 10, "output_tokens": 25},
    }

    # First attempts fail with status code 429, 2nd succeeds
    def create_response(status_code, json_data=None):
        response = httpx.Response(status_code)
        response._content = httpx._content.json_dumps(json_data or {}).encode()
        response._request = httpx.Request("POST", "https://api.anthropic.com/v1/messages")
        return response

    mock_post.side_effect = [
        create_response(429),  # 1st attempt
        create_response(200, mock_response),  # Final success
    ]

    model = "claude-3-5-sonnet-20240620"
    system = "You are a helpful assistant."
    messages = [Message.user("Hello, Claude")]

    reply_message, reply_usage = anthropic_provider.complete(model=model, system=system, messages=messages)

    assert reply_message.content == [Text(text="Hello from Claude!")]
    assert reply_usage.total_tokens == 35
    assert mock_post.call_count == 2
    mock_post.assert_any_call(
        "https://api.anthropic.com/v1/messages",
        json={
            "system": system,
            "model": model,
            "max_tokens": 4096,
            "messages": [
                *[
                    {
                        "role": msg.role,
                        "content": [{"type": "text", "text": msg.content[0].text}],
                    }
                    for msg in messages
                ],
            ],
        },
    )


@pytest.mark.integration
def test_anthropic_integration():
    provider = AnthropicProvider.from_env()
    model = "claude-3-5-sonnet-20240620"  # updated model to a known valid model
    system = "You are a helpful assistant."
    messages = [Message.user("Hello, Claude")]

    # Run the completion
    reply = provider.complete(model=model, system=system, messages=messages)

    assert reply[0].content is not None
    print("Completion content from Anthropic:", reply[0].content)
