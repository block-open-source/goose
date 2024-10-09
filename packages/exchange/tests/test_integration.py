import os
import pytest
from exchange.exchange import Exchange
from exchange.message import Message
from exchange.moderators import ContextTruncate
from exchange.providers import get_provider
from exchange.providers.ollama import OLLAMA_MODEL
from exchange.tool import Tool
from tests.conftest import read_file

too_long_chars = "x" * (2**20 + 1)

cases = [
    # Set seed and temperature for more determinism, to avoid flakes
    (get_provider("ollama"), os.getenv("OLLAMA_MODEL", OLLAMA_MODEL), dict(seed=3, temperature=0.1)),
    (get_provider("openai"), os.getenv("OPENAI_MODEL", "gpt-4o-mini"), dict()),
    (get_provider("azure"), os.getenv("AZURE_MODEL", "gpt-4o-mini"), dict()),
    (get_provider("databricks"), "databricks-meta-llama-3-70b-instruct", dict()),
    (get_provider("bedrock"), "anthropic.claude-3-5-sonnet-20240620-v1:0", dict()),
    (get_provider("google"), "gemini-1.5-flash", dict()),
]


@pytest.mark.integration
@pytest.mark.parametrize("provider,model,kwargs", cases)
def test_simple(provider, model, kwargs):
    provider = provider.from_env()

    ex = Exchange(
        provider=provider,
        model=model,
        moderator=ContextTruncate(model),
        system="You are a helpful assistant.",
        generation_args=kwargs,
    )

    ex.add(Message.user("Who is the most famous wizard from the lord of the rings"))

    response = ex.reply()

    # It's possible this can be flakey, but in experience so far haven't seen it
    assert "gandalf" in response.text.lower()


@pytest.mark.integration
@pytest.mark.parametrize("provider,model,kwargs", cases)
def test_tools(provider, model, kwargs, tmp_path):
    provider = provider.from_env()

    ex = Exchange(
        provider=provider,
        model=model,
        moderator=ContextTruncate(model),
        system="You are a helpful assistant. Expect to need to read a file using read_file.",
        tools=(Tool.from_function(read_file),),
        generation_args=kwargs,
    )

    ex.add(Message.user("What are the contents of this file? test.txt"))

    response = ex.reply()

    assert "hello exchange" in response.text.lower()


@pytest.mark.integration
@pytest.mark.parametrize("provider,model,kwargs", cases)
def test_tool_use_output_chars(provider, model, kwargs):
    provider = provider.from_env()

    def get_password() -> str:
        """Return the password for authentication"""
        return too_long_chars

    ex = Exchange(
        provider=provider,
        model=model,
        moderator=ContextTruncate(model),
        system="You are a helpful assistant. Expect to need to authenticate using get_password.",
        tools=(Tool.from_function(get_password),),
        generation_args=kwargs,
    )

    ex.add(Message.user("Can you authenticate this session by responding with the password"))

    ex.reply()

    # Without our error handling, this would raise
    # string too long. Expected a string with maximum length 1048576, but got a string with length ...
