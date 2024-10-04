import logging
import os
from unittest.mock import patch

import pytest
from exchange.content import Text, ToolResult, ToolUse
from exchange.message import Message
from exchange.providers.base import MissingProviderEnvVariableError
from exchange.providers.bedrock import BedrockProvider
from exchange.tool import Tool

logger = logging.getLogger(__name__)


@pytest.mark.parametrize(
    "env_var_name",
    [
        ("AWS_ACCESS_KEY_ID"),
        ("AWS_SECRET_ACCESS_KEY"),
        ("AWS_SESSION_TOKEN"),
    ],
)
def test_from_env_throw_error_when_missing_env_var(env_var_name):
    with patch.dict(
        os.environ,
        {
            "AWS_ACCESS_KEY_ID": "test_access_key_id",
            "AWS_SECRET_ACCESS_KEY": "test_secret_access_key",
            "AWS_SESSION_TOKEN": "test_session_token",
        },
        clear=True,
    ):
        os.environ.pop(env_var_name)
        with pytest.raises(MissingProviderEnvVariableError) as context:
            BedrockProvider.from_env()
        assert context.value.provider == "bedrock"
        assert context.value.env_variable == env_var_name
        assert context.value.message == f"Missing environment variable: {env_var_name} for provider bedrock."


@pytest.fixture
@patch.dict(
    os.environ,
    {
        "AWS_REGION": "us-east-1",
        "AWS_ACCESS_KEY_ID": "fake-access-key",
        "AWS_SECRET_ACCESS_KEY": "fake-secret-key",
        "AWS_SESSION_TOKEN": "fake-session-token",
    },
)
def bedrock_provider():
    return BedrockProvider.from_env()


@patch("time.time", return_value=1624250000)
def test_sign_and_get_headers(mock_time, bedrock_provider):
    # Create sample values
    method = "POST"
    url = "https://bedrock-runtime.us-east-1.amazonaws.com/some/path"
    payload = {"key": "value"}
    service = "bedrock"
    # Generate headers
    headers = bedrock_provider.client.sign_and_get_headers(
        method,
        url,
        payload,
        service,
    )
    # Assert that headers contain expected keys
    assert "Authorization" in headers
    assert "Content-Type" in headers
    assert "X-Amz-date" in headers
    assert "x-amz-content-sha256" in headers
    assert "X-Amz-Security-Token" in headers


@patch("httpx.Client.post")
def test_complete(mock_post, bedrock_provider):
    # Mocked response from the server
    mock_response = {
        "output": {"message": {"role": "assistant", "content": [{"text": "Hello, world!"}]}},
        "usage": {"inputTokens": 10, "outputTokens": 15, "totalTokens": 25},
    }
    mock_post.return_value.json.return_value = mock_response

    model = "test-model"
    system = "You are a helpful assistant."
    messages = [Message.user("Hello")]
    tools = ()

    reply_message, reply_usage = bedrock_provider.complete(model=model, system=system, messages=messages, tools=tools)

    # Assertions for reply message
    assert reply_message.content[0].text == "Hello, world!"
    assert reply_usage.total_tokens == 25


def test_message_to_bedrock_spec_text(bedrock_provider):
    message = Message(role="user", content=[Text("Hello, world!")])
    expected = {"role": "user", "content": [{"text": "Hello, world!"}]}
    assert bedrock_provider.message_to_bedrock_spec(message) == expected


def test_message_to_bedrock_spec_tool_use(bedrock_provider):
    tool_use = ToolUse(id="tool-1", name="WordCount", parameters={"text": "Hello, world!"})
    message = Message(role="assistant", content=[tool_use])
    expected = {
        "role": "assistant",
        "content": [
            {
                "toolUse": {
                    "toolUseId": "tool-1",
                    "name": "WordCount",
                    "input": {"text": "Hello, world!"},
                }
            }
        ],
    }
    assert bedrock_provider.message_to_bedrock_spec(message) == expected


def test_message_to_bedrock_spec_tool_result(bedrock_provider):
    message = Message(
        role="assistant",
        content=[ToolUse(id="tool-1", name="WordCount", parameters={"text": "Hello, world!"})],
    )
    expected = {
        "role": "assistant",
        "content": [
            {
                "toolUse": {
                    "toolUseId": "tool-1",
                    "name": "WordCount",
                    "input": {"text": "Hello, world!"},
                }
            }
        ],
    }
    assert bedrock_provider.message_to_bedrock_spec(message) == expected


def test_message_to_bedrock_spec_tool_result_text(bedrock_provider):
    tool_result = ToolResult(tool_use_id="tool-1", output="Error occurred", is_error=True)
    message = Message(role="user", content=[tool_result])
    expected = {
        "role": "user",
        "content": [
            {
                "toolResult": {
                    "toolUseId": "tool-1",
                    "content": [{"text": "Error occurred"}],
                    "status": "error",
                }
            }
        ],
    }
    assert bedrock_provider.message_to_bedrock_spec(message) == expected


def test_message_to_bedrock_spec_invalid(bedrock_provider):
    with pytest.raises(Exception):
        bedrock_provider.message_to_bedrock_spec(Message(role="user", content=[]))


def test_response_to_message_text(bedrock_provider):
    response = {"role": "user", "content": [{"text": "Hello, world!"}]}
    message = bedrock_provider.response_to_message(response)
    assert message.role == "user"
    assert message.content[0].text == "Hello, world!"


def test_response_to_message_tool_use(bedrock_provider):
    response = {
        "role": "assistant",
        "content": [
            {
                "toolUse": {
                    "toolUseId": "tool-1",
                    "name": "WordCount",
                    "input": {"text": "Hello, world!"},
                }
            }
        ],
    }
    message = bedrock_provider.response_to_message(response)
    assert message.role == "assistant"
    assert message.content[0].name == "WordCount"
    assert message.content[0].parameters == {"text": "Hello, world!"}


def test_response_to_message_tool_result(bedrock_provider):
    response = {
        "role": "user",
        "content": [
            {
                "toolResult": {
                    "toolResultId": "tool-1",
                    "content": [{"json": {"result": 2}}],
                }
            }
        ],
    }
    message = bedrock_provider.response_to_message(response)
    assert message.role == "user"
    assert message.content[0].tool_use_id == "tool-1"
    assert message.content[0].output == {"result": 2}


def test_response_to_message_invalid(bedrock_provider):
    with pytest.raises(Exception):
        bedrock_provider.response_to_message({})


def test_tools_to_bedrock_spec(bedrock_provider):
    def word_count(text: str):
        return len(text.split())

    tool = Tool(
        name="WordCount",
        description="Counts words.",
        parameters={"text": "string"},
        function=word_count,
    )
    expected = {
        "tools": [
            {
                "toolSpec": {
                    "name": "WordCount",
                    "description": "Counts words.",
                    "inputSchema": {"json": {"text": "string"}},
                }
            }
        ]
    }
    assert bedrock_provider.tools_to_bedrock_spec((tool,)) == expected


def test_tools_to_bedrock_spec_duplicate(bedrock_provider):
    def word_count(text: str):
        return len(text.split())

    tool = Tool(
        name="WordCount",
        description="Counts words.",
        parameters={"text": "string"},
        function=word_count,
    )
    tool_duplicate = Tool(
        name="WordCount",
        description="Counts words.",
        parameters={"text": "string"},
        function=word_count,
    )
    tools = bedrock_provider.tools_to_bedrock_spec((tool, tool_duplicate))
    assert set(tool["toolSpec"]["name"] for tool in tools["tools"]) == {"WordCount"}
