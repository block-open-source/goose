import os
import re
from typing import Type, Tuple

import pytest

from exchange import Message, ToolUse, ToolResult, Tool
from exchange.providers import Usage, Provider
from tests.conftest import read_file

OPENAI_API_KEY = "test_openai_api_key"
OPENAI_ORG_ID = "test_openai_org_key"
OPENAI_PROJECT_ID = "test_openai_project_id"


@pytest.fixture
def default_openai_env(monkeypatch):
    """
    This fixture prevents OpenAIProvider.from_env() from erring on missing
    environment variables.

    When running VCR tests for the first time or after deleting a cassette
    recording, set required environment variables, so that real requests don't
    fail. Subsequent runs use the recorded data, so don't need them.
    """
    if "OPENAI_API_KEY" not in os.environ:
        monkeypatch.setenv("OPENAI_API_KEY", OPENAI_API_KEY)


AZURE_ENDPOINT = "https://test.openai.azure.com"
AZURE_DEPLOYMENT_NAME = "test-azure-deployment"
AZURE_API_VERSION = "2024-05-01-preview"
AZURE_API_KEY = "test_azure_api_key"


@pytest.fixture
def default_azure_env(monkeypatch):
    """
    This fixture prevents AzureProvider.from_env() from erring on missing
    environment variables.

    When running VCR tests for the first time or after deleting a cassette
    recording, set required environment variables, so that real requests don't
    fail. Subsequent runs use the recorded data, so don't need them.
    """
    if "AZURE_CHAT_COMPLETIONS_HOST_NAME" not in os.environ:
        monkeypatch.setenv("AZURE_CHAT_COMPLETIONS_HOST_NAME", AZURE_ENDPOINT)
    if "AZURE_CHAT_COMPLETIONS_DEPLOYMENT_NAME" not in os.environ:
        monkeypatch.setenv("AZURE_CHAT_COMPLETIONS_DEPLOYMENT_NAME", AZURE_DEPLOYMENT_NAME)
    if "AZURE_CHAT_COMPLETIONS_DEPLOYMENT_API_VERSION" not in os.environ:
        monkeypatch.setenv("AZURE_CHAT_COMPLETIONS_DEPLOYMENT_API_VERSION", AZURE_API_VERSION)
    if "AZURE_CHAT_COMPLETIONS_KEY" not in os.environ:
        monkeypatch.setenv("AZURE_CHAT_COMPLETIONS_KEY", AZURE_API_KEY)


@pytest.fixture(scope="module")
def vcr_config():
    """
    This scrubs sensitive data and gunzips bodies when in recording mode.

    Without this, you would leak cookies and auth tokens in the cassettes.
    Also, depending on the request, some responses would be binary encoded
    while others plain json. This ensures all bodies are human-readable.
    """
    return {
        "decode_compressed_response": True,
        "filter_headers": [
            ("authorization", "Bearer " + OPENAI_API_KEY),
            ("openai-organization", OPENAI_ORG_ID),
            ("openai-project", OPENAI_PROJECT_ID),
            ("cookie", None),
        ],
        "before_record_request": scrub_request_url,
        "before_record_response": scrub_response_headers,
    }


def scrub_request_url(request):
    """
    This scrubs sensitive request data in provider-specific way. Note that headers
    are case-sensitive!
    """
    if "openai.azure.com" in request.uri:
        request.uri = re.sub(r"https://[^/]+", AZURE_ENDPOINT, request.uri)
        request.uri = re.sub(r"/deployments/[^/]+", f"/deployments/{AZURE_DEPLOYMENT_NAME}", request.uri)
        request.headers["host"] = AZURE_ENDPOINT.replace("https://", "")
        request.headers["api-key"] = AZURE_API_KEY

    return request


def scrub_response_headers(response):
    """
    This scrubs sensitive response headers. Note they are case-sensitive!
    """
    response["headers"]["openai-organization"] = OPENAI_ORG_ID
    response["headers"]["Set-Cookie"] = "test_set_cookie"
    return response


def complete(provider_cls: Type[Provider], model: str) -> Tuple[Message, Usage]:
    provider = provider_cls.from_env()
    system = "You are a helpful assistant."
    messages = [Message.user("Hello")]
    return provider.complete(model=model, system=system, messages=messages, tools=None)


def tools(provider_cls: Type[Provider], model: str) -> Tuple[Message, Usage]:
    provider = provider_cls.from_env()
    system = "You are a helpful assistant. Expect to need to read a file using read_file."
    messages = [Message.user("What are the contents of this file? test.txt")]
    return provider.complete(model=model, system=system, messages=messages, tools=(Tool.from_function(read_file),))


def vision(provider_cls: Type[Provider], model: str) -> Tuple[Message, Usage]:
    provider = provider_cls.from_env()
    system = "You are a helpful assistant."
    messages = [
        Message.user("What does the first entry in the menu say?"),
        Message(
            role="assistant",
            content=[ToolUse(id="xyz", name="screenshot", parameters={})],
        ),
        Message(
            role="user",
            content=[ToolResult(tool_use_id="xyz", output='"image:tests/test_image.png"')],
        ),
    ]
    return provider.complete(model=model, system=system, messages=messages, tools=None)
