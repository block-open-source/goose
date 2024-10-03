import os
from unittest.mock import patch

import pytest

from exchange import Text, ToolUse
from exchange.providers.azure import AzureProvider
from exchange.providers.base import MissingProviderEnvVariableError
from .conftest import complete, tools

AZURE_MODEL = os.getenv("AZURE_MODEL", "gpt-4o-mini")


@pytest.mark.parametrize(
    "env_var_name",
    [
        ("AZURE_CHAT_COMPLETIONS_HOST_NAME"),
        ("AZURE_CHAT_COMPLETIONS_DEPLOYMENT_NAME"),
        ("AZURE_CHAT_COMPLETIONS_DEPLOYMENT_API_VERSION"),
        ("AZURE_CHAT_COMPLETIONS_KEY"),
    ],
)
def test_from_env_throw_error_when_missing_env_var(env_var_name):
    with patch.dict(
        os.environ,
        {
            "AZURE_CHAT_COMPLETIONS_HOST_NAME": "test_host_name",
            "AZURE_CHAT_COMPLETIONS_DEPLOYMENT_NAME": "test_deployment_name",
            "AZURE_CHAT_COMPLETIONS_DEPLOYMENT_API_VERSION": "test_api_version",
            "AZURE_CHAT_COMPLETIONS_KEY": "test_api_key",
        },
        clear=True,
    ):
        os.environ.pop(env_var_name)
        with pytest.raises(MissingProviderEnvVariableError) as context:
            AzureProvider.from_env()
        assert context.value.provider == "azure"
        assert context.value.env_variable == env_var_name
        assert context.value.message == f"Missing environment variable: {env_var_name} for provider azure."


@pytest.mark.vcr()
def test_azure_complete(default_azure_env):
    reply_message, reply_usage = complete(AzureProvider, AZURE_MODEL)

    assert reply_message.content == [Text(text="Hello! How can I assist you today?")]
    assert reply_usage.total_tokens == 27


@pytest.mark.integration
def test_azure_complete_integration():
    reply = complete(AzureProvider, AZURE_MODEL)

    assert reply[0].content is not None
    print("Completion content from Azure:", reply[0].content)


@pytest.mark.vcr()
def test_azure_tools(default_azure_env):
    reply_message, reply_usage = tools(AzureProvider, AZURE_MODEL)

    tool_use = reply_message.content[0]
    assert isinstance(tool_use, ToolUse), f"Expected ToolUse, but was {type(tool_use).__name__}"
    assert tool_use.id == "call_a47abadDxlGKIWjvYYvGVAHa"
    assert tool_use.name == "read_file"
    assert tool_use.parameters == {"filename": "test.txt"}
    assert reply_usage.total_tokens == 125


@pytest.mark.integration
def test_azure_tools_integration():
    reply = tools(AzureProvider, AZURE_MODEL)

    tool_use = reply[0].content[0]
    assert isinstance(tool_use, ToolUse), f"Expected ToolUse, but was {type(tool_use).__name__}"
    assert tool_use.id is not None
    assert tool_use.name == "read_file"
    assert tool_use.parameters == {"filename": "test.txt"}
