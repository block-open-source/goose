import os
from typing import Type

import httpx

from exchange.providers import OpenAiProvider


class AzureProvider(OpenAiProvider):
    """Provides chat completions for models hosted by the Azure OpenAI Service"""

    def __init__(self, client: httpx.Client) -> None:
        super().__init__(client)

    @classmethod
    def from_env(cls: Type["AzureProvider"]) -> "AzureProvider":
        try:
            url = os.environ["AZURE_CHAT_COMPLETIONS_HOST_NAME"]
        except KeyError:
            raise RuntimeError("Failed to get AZURE_CHAT_COMPLETIONS_HOST_NAME from the environment.")

        try:
            deployment_name = os.environ["AZURE_CHAT_COMPLETIONS_DEPLOYMENT_NAME"]
        except KeyError:
            raise RuntimeError("Failed to get AZURE_CHAT_COMPLETIONS_DEPLOYMENT_NAME from the environment.")

        try:
            api_version = os.environ["AZURE_CHAT_COMPLETIONS_DEPLOYMENT_API_VERSION"]
        except KeyError:
            raise RuntimeError("Failed to get AZURE_CHAT_COMPLETIONS_DEPLOYMENT_API_VERSION from the environment.")

        try:
            key = os.environ["AZURE_CHAT_COMPLETIONS_KEY"]
        except KeyError:
            raise RuntimeError("Failed to get AZURE_CHAT_COMPLETIONS_KEY from the environment.")

        # format the url host/"openai/deployments/" + deployment_name + "/?api-version=" + api_version
        url = f"{url}/openai/deployments/{deployment_name}/"
        client = httpx.Client(
            base_url=url,
            headers={"api-key": key, "Content-Type": "application/json"},
            params={"api-version": api_version},
            timeout=httpx.Timeout(60 * 10),
        )
        return cls(client)
