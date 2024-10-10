from functools import cache
from typing import Type

from exchange.invalid_choice_error import InvalidChoiceError
from exchange.providers.anthropic import AnthropicProvider  # noqa
from exchange.providers.base import Provider, Usage  # noqa
from exchange.providers.databricks import DatabricksProvider  # noqa
from exchange.providers.openai import OpenAiProvider  # noqa
from exchange.providers.ollama import OllamaProvider  # noqa
from exchange.providers.groq import GroqProvider  # noqa
from exchange.providers.azure import AzureProvider  # noqa
from exchange.providers.google import GoogleProvider  # noqa

from exchange.utils import load_plugins


@cache
def get_provider(name: str) -> Type[Provider]:
    providers = load_plugins(group="exchange.provider")
    if name not in providers:
        raise InvalidChoiceError("provider", name, providers.keys())
    return providers[name]
