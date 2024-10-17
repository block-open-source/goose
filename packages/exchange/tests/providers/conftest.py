import json
import os
import re

import pytest
import yaml

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


GOOGLE_API_KEY = "test_google_api_key"


@pytest.fixture
def default_google_env(monkeypatch):
    """
    This fixture prevents GoogleProvider.from_env() from erring on missing
    environment variables.

    When running VCR tests for the first time or after deleting a cassette
    recording, set required environment variables, so that real requests don't
    fail. Subsequent runs use the recorded data, so don't need them.
    """
    if "GOOGLE_API_KEY" not in os.environ:
        monkeypatch.setenv("GOOGLE_API_KEY", GOOGLE_API_KEY)


class LiteralBlockScalar(str):
    """Formats the string as a literal block scalar, preserving whitespace and
    without interpreting escape characters"""

    pass


def literal_block_scalar_presenter(dumper, data):
    """Represents a scalar string as a literal block, via '|' syntax"""
    return dumper.represent_scalar("tag:yaml.org,2002:str", data, style="|")


yaml.add_representer(LiteralBlockScalar, literal_block_scalar_presenter)


def process_string_value(string_value):
    """Pretty-prints JSON or returns long strings as a LiteralString"""
    try:
        json_data = json.loads(string_value)
        return LiteralBlockScalar(json.dumps(json_data, indent=2))
    except (ValueError, TypeError):
        if len(string_value) > 80:
            return LiteralBlockScalar(string_value)
    return string_value


def convert_body_to_literal(data):
    """Searches the data for body strings, attempting to pretty-print JSON"""
    if isinstance(data, dict):
        for key, value in data.items():
            # Handle response body case (e.g., response.body.string)
            if key == "body" and isinstance(value, dict) and "string" in value:
                value["string"] = process_string_value(value["string"])

            # Handle request body case (e.g., request.body)
            elif key == "body" and isinstance(value, str):
                data[key] = process_string_value(value)

            else:
                convert_body_to_literal(value)

    elif isinstance(data, list):
        for i, item in enumerate(data):
            data[i] = convert_body_to_literal(item)

    return data


class PrettyPrintJSONBody:
    """This makes request and response body recordings more readable."""

    @staticmethod
    def serialize(cassette_dict):
        cassette_dict = convert_body_to_literal(cassette_dict)
        return yaml.dump(cassette_dict, default_flow_style=False, allow_unicode=True)

    @staticmethod
    def deserialize(cassette_string):
        return yaml.load(cassette_string, Loader=yaml.Loader)


@pytest.fixture(scope="module")
def vcr(vcr):
    vcr.register_serializer("yaml", PrettyPrintJSONBody)
    return vcr


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
    elif "generativelanguage.googleapis.com" in request.uri:
        request.uri = re.sub(r"([?&])key=[^&]+", r"\1key=" + GOOGLE_API_KEY, request.uri)

    return request


def scrub_response_headers(response):
    """
    This scrubs sensitive response headers. Note they are case-sensitive!
    """
    if "openai-organization" in response["headers"]:
        response["headers"]["openai-organization"] = OPENAI_ORG_ID
    if "Set-Cookie" in response["headers"]:
        response["headers"]["Set-Cookie"] = "test_set_cookie"
    return response


def complete(provider_cls: type[Provider], model: str, **kwargs) -> tuple[Message, Usage]:
    provider = provider_cls.from_env()
    system = "You are a helpful assistant."
    messages = [Message.user("Hello")]
    return provider.complete(model=model, system=system, messages=messages, tools=(), **kwargs)


def tools(provider_cls: type[Provider], model: str, **kwargs) -> tuple[Message, Usage]:
    provider = provider_cls.from_env()
    system = "You are a helpful assistant. Expect to need to read a file using read_file."
    messages = [Message.user("What are the contents of this file? test.txt")]
    return provider.complete(
        model=model, system=system, messages=messages, tools=(Tool.from_function(read_file),), **kwargs
    )


def vision(provider_cls: type[Provider], model: str, **kwargs) -> tuple[Message, Usage]:
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
    return provider.complete(model=model, system=system, messages=messages, tools=(), **kwargs)
