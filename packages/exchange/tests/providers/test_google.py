import os
from unittest.mock import patch

import httpx
import pytest
from exchange import Message, Text
from exchange.content import ToolResult, ToolUse
from exchange.providers.google import GoogleProvider
from exchange.tool import Tool


def example_fn(param: str) -> None:
    """
    Testing function.

    Args:
        param (str): Description of param1
    """
    pass


@pytest.fixture
@patch.dict(os.environ, {"GOOGLE_API_KEY": "test_api_key"})
def google_provider():
    return GoogleProvider.from_env()


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
        Message(role="user", content=[Text(text="Hello, Gemini")]),
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


@patch("httpx.Client.post")
@patch("logging.warning")
@patch("logging.error")
def test_google_completion(mock_error, mock_warning, mock_post, google_provider):
    mock_response = {
        "candidates": [{"content": {"parts": [{"text": "Hello from Gemini!"}], "role": "model"}}],
        "usageMetadata": {"promptTokenCount": 3, "candidatesTokenCount": 10, "totalTokenCount": 13},
    }

    # First attempts fail with status code 429, 2nd succeeds
    def create_response(status_code, json_data=None):
        response = httpx.Response(status_code)
        response._content = httpx._content.json_dumps(json_data or {}).encode()
        response._request = httpx.Request("POST", "https://generativelanguage.googleapis.com/v1beta/")
        return response

    mock_post.side_effect = [
        create_response(429),  # 1st attempt
        create_response(200, mock_response),  # Final success
    ]

    model = "gemini-1.5-flash"
    system = "You are a helpful assistant."
    messages = [Message.user("Hello, Gemini")]

    reply_message, reply_usage = google_provider.complete(model=model, system=system, messages=messages)

    assert reply_message.content == [Text(text="Hello from Gemini!")]
    assert reply_usage.total_tokens == 13
    assert mock_post.call_count == 2
    mock_post.assert_any_call(
        "models/gemini-1.5-flash:generateContent",
        json={
            "system_instruction": {"parts": [{"text": "You are a helpful assistant."}]},
            "contents": [{"role": "user", "parts": [{"text": "Hello, Gemini"}]}],
        },
    )


@pytest.mark.integration
def test_google_integration():
    provider = GoogleProvider.from_env()
    model = "gemini-1.5-flash"  # updated model to a known valid model
    system = "You are a helpful assistant."
    messages = [Message.user("Hello, Gemini")]

    # Run the completion
    reply = provider.complete(model=model, system=system, messages=messages)

    assert reply[0].content is not None
    print("Completion content from Google:", reply[0].content)
