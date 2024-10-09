import os
from unittest.mock import patch

import pytest
from exchange import Message, Text
from exchange.providers.nvidia import NVIDIAProvider


@pytest.fixture
@patch.dict(os.environ, {"NVIDIA_API_KEY": "dummy_key"})
def nvidia_provider():
    return NVIDIAProvider.from_env()


@patch("httpx.Client.post")
@patch("time.sleep", return_value=None)
@patch("logging.warning")
@patch("logging.error")
def test_nvidia_completion(mock_error, mock_warning, mock_sleep, mock_post, nvidia_provider):
    mock_response = {
        "choices": [{"message": {"role": "assistant", "content": "Hello!"}}],
        "usage": {"prompt_tokens": 10, "completion_tokens": 25, "total_tokens": 35},
    }

    mock_post.return_value.json.return_value = mock_response

    model = "meta/llama-3.1-8b-instruct"
    system = "You are a helpful assistant."
    messages = [Message.user("Hello")]
    tools = ()

    reply_message, reply_usage = nvidia_provider.complete(model=model, system=system, messages=messages, tools=tools)

    assert reply_message.content == [Text(text="Hello!")]
    assert reply_usage.total_tokens == 35
    mock_post.assert_called_once_with(
        "v1/chat/completions",
        json={
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": "Hello"},
            ],
            "model": model,
        },
    )

@pytest.mark.integration
def test_nvidia_integration():
    provider = NVIDIAProvider.from_env()
    model = "meta/llama-3.1-8b-instruct"
    system = "You are a helpful assistant."
    messages = [Message.user("Hello")]

    reply = provider.complete(model=model, system=system, messages=messages, tools=None)

    assert reply[0].content is not None
    print("Completion content from NVIDIA:", reply[0].content)