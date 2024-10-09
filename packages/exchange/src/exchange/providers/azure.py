from typing import Type

import httpx
import os

from exchange.providers import OpenAiProvider


class AzureProvider(OpenAiProvider):
    """Provides chat completions for models hosted by the Azure OpenAI Service."""

    PROVIDER_NAME = "azure"
    REQUIRED_ENV_VARS = [
        "AZURE_CHAT_COMPLETIONS_HOST_NAME",
        "AZURE_CHAT_COMPLETIONS_DEPLOYMENT_NAME",
        "AZURE_CHAT_COMPLETIONS_DEPLOYMENT_API_VERSION",
        "AZURE_CHAT_COMPLETIONS_KEY",
    ]

    def __init__(self, client: httpx.Client) -> None:
        super().__init__(client)

    @classmethod
    def from_env(cls: Type["AzureProvider"]) -> "AzureProvider":
        cls.check_env_vars()
        url = os.environ.get("AZURE_CHAT_COMPLETIONS_HOST_NAME")
        deployment_name = os.environ.get("AZURE_CHAT_COMPLETIONS_DEPLOYMENT_NAME")
        api_version = os.environ.get("AZURE_CHAT_COMPLETIONS_DEPLOYMENT_API_VERSION")
        key = os.environ.get("AZURE_CHAT_COMPLETIONS_KEY")

        # format the url host/"openai/deployments/" + deployment_name + "/?api-version=" + api_version
        url = f"{url}/openai/deployments/{deployment_name}/"
        client = httpx.Client(
            base_url=url,
            headers={"api-key": key, "Content-Type": "application/json"},
            params={"api-version": api_version},
            timeout=httpx.Timeout(60 * 10),
        )
        return cls(client)
