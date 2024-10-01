import os
from typing import Any, Dict, List, Tuple, Type

import httpx

from exchange.message import Message
from exchange.providers.base import Provider, Usage
from tenacity import retry, wait_fixed, stop_after_attempt
from exchange.providers.utils import raise_for_status, retry_if_status
from exchange.providers.utils import (
    messages_to_openai_spec,
    openai_response_to_message,
    tools_to_openai_spec,
)
from exchange.tool import Tool


retry_procedure = retry(
    wait=wait_fixed(2),
    stop=stop_after_attempt(2),
    retry=retry_if_status(codes=[429], above=500),
    reraise=True,
)


class DatabricksProvider(Provider):
    """Provides chat completions for models on Databricks serving endpoints

    Models are expected to follow the llm/v1/chat "task". This includes support
    for foundation and external model endpoints
    https://docs.databricks.com/en/machine-learning/model-serving/create-foundation-model-endpoints.html#create-generative-ai-model-serving-endpoints
    """

    def __init__(self, client: httpx.Client) -> None:
        super().__init__()
        self.client = client

    @classmethod
    def from_env(cls: Type["DatabricksProvider"]) -> "DatabricksProvider":
        try:
            url = os.environ["DATABRICKS_HOST"]
        except KeyError:
            raise RuntimeError(
                "Failed to get DATABRICKS_HOST from the environment. See https://docs.databricks.com/en/dev-tools/auth/index.html#general-host-token-and-account-id-environment-variables-and-fields"
            )
        try:
            key = os.environ["DATABRICKS_TOKEN"]
        except KeyError:
            raise RuntimeError(
                "Failed to get DATABRICKS_TOKEN from the environment. See https://docs.databricks.com/en/dev-tools/auth/index.html#general-host-token-and-account-id-environment-variables-and-fields"
            )
        client = httpx.Client(
            base_url=url,
            auth=("token", key),
            timeout=httpx.Timeout(60 * 10),
        )
        return cls(client)

    @staticmethod
    def get_usage(data: dict) -> Usage:
        usage = data.pop("usage")
        input_tokens = usage.get("prompt_tokens")
        output_tokens = usage.get("completion_tokens")
        total_tokens = usage.get("total_tokens")
        if total_tokens is None and input_tokens is not None and output_tokens is not None:
            total_tokens = input_tokens + output_tokens

        return Usage(
            input_tokens=input_tokens,
            output_tokens=output_tokens,
            total_tokens=total_tokens,
        )

    def complete(
        self,
        model: str,
        system: str,
        messages: List[Message],
        tools: Tuple[Tool],
        **kwargs: Dict[str, Any],
    ) -> Tuple[Message, Usage]:
        payload = dict(
            messages=[
                {"role": "system", "content": system},
                *messages_to_openai_spec(messages),
            ],
            tools=tools_to_openai_spec(tools) if tools else [],
            **kwargs,
        )
        payload = {k: v for k, v in payload.items() if v}
        response = self._post(model, payload)
        message = openai_response_to_message(response)
        usage = self.get_usage(response)
        return message, usage

    @retry_procedure
    def _post(self, model: str, payload: dict) -> httpx.Response:
        response = self.client.post(
            f"serving-endpoints/{model}/invocations",
            json=payload,
        )
        return raise_for_status(response).json()
