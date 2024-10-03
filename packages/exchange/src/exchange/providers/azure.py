from typing import Type

import httpx

from exchange.providers import OpenAiProvider
from exchange.providers.utils import get_provider_env_value


class AzureProvider(OpenAiProvider):
    """Provides chat completions for models hosted by the Azure OpenAI Service"""

    def __init__(self, client: httpx.Client) -> None:
        super().__init__(client)

    @classmethod
    def from_env(cls: Type["AzureProvider"]) -> "AzureProvider":
        url = cls._get_env_variable("AZURE_CHAT_COMPLETIONS_HOST_NAME")
        deployment_name = cls._get_env_variable("AZURE_CHAT_COMPLETIONS_DEPLOYMENT_NAME")

        api_version = cls._get_env_variable("AZURE_CHAT_COMPLETIONS_DEPLOYMENT_API_VERSION")
        key = cls._get_env_variable("AZURE_CHAT_COMPLETIONS_KEY")

        # format the url host/"openai/deployments/" + deployment_name + "/?api-version=" + api_version
        url = f"{url}/openai/deployments/{deployment_name}/"
        client = httpx.Client(
            base_url=url,
            headers={"api-key": key, "Content-Type": "application/json"},
            params={"api-version": api_version},
            timeout=httpx.Timeout(60 * 10),
        )
        return cls(client)

    @classmethod
    def _get_env_variable(cls: Type["AzureProvider"], key: str) -> str:
        return get_provider_env_value(key, "azure")
