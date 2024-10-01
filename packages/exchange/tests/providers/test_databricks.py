import os
from unittest.mock import patch

import pytest
from exchange import Message, Text
from exchange.providers.databricks import DatabricksProvider


@pytest.fixture
@patch.dict(
    os.environ,
    {"DATABRICKS_HOST": "http://test-host", "DATABRICKS_TOKEN": "test_token"},
)
def databricks_provider():
    return DatabricksProvider.from_env()


@patch("httpx.Client.post")
@patch("time.sleep", return_value=None)
@patch("logging.warning")
@patch("logging.error")
def test_databricks_completion(mock_error, mock_warning, mock_sleep, mock_post, databricks_provider):
    mock_response = {
        "choices": [{"message": {"role": "assistant", "content": "Hello!"}}],
        "usage": {"prompt_tokens": 10, "completion_tokens": 25, "total_tokens": 35},
    }
    mock_post.return_value.json.return_value = mock_response

    model = "my-databricks-model"
    system = "You are a helpful assistant."
    messages = [Message.user("Hello")]
    tools = ()

    reply_message, reply_usage = databricks_provider.complete(
        model=model, system=system, messages=messages, tools=tools
    )

    assert reply_message.content == [Text(text="Hello!")]
    assert reply_usage.total_tokens == 35
    assert mock_post.call_count == 1
    mock_post.assert_called_once_with(
        "serving-endpoints/my-databricks-model/invocations",
        json={
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": "Hello"},
            ]
        },
    )
