import os
from typing import Any, Dict, List, Tuple, Type

import httpx

from exchange.message import Message
from exchange.providers.base import Provider, Usage
from exchange.providers.utils import (
    messages_to_openai_spec,
    openai_response_to_message,
    openai_single_message_context_length_exceeded,
    raise_for_status,
    tools_to_openai_spec,
)
from exchange.tool import Tool
from tenacity import retry, wait_fixed, stop_after_attempt
from exchange.providers.utils import retry_if_status

GROQ_HOST = "https://api.groq.com/openai/"

retry_procedure = retry(
    wait=wait_fixed(5),
    stop=stop_after_attempt(5),
    retry=retry_if_status(codes=[429], above=500),
    reraise=True,
)


class GroqProvider(Provider):
    """Provides chat completions for models hosted directly by OpenAI."""

    PROVIDER_NAME = "groq"
    REQUIRED_ENV_VARS = ["GROQ_API_KEY"]
    instructions_url = "https://console.groq.com/docs/quickstart"

    def __init__(self, client: httpx.Client) -> None:
        self.client = client

    @classmethod
    def from_env(cls: Type["GroqProvider"]) -> "GroqProvider":
        cls.check_env_vars(cls.instructions_url)
        url = os.environ.get("GROQ_HOST", GROQ_HOST)
        key = os.environ.get("GROQ_API_KEY")

        client = httpx.Client(
            base_url=url + "v1/",
            headers={"Authorization": "Bearer " + key},
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
        system_message = [{"role": "system", "content": system}]
        payload = dict(
            messages=system_message + messages_to_openai_spec(messages),
            model=model,
            tools=tools_to_openai_spec(tools) if tools else [],
            **kwargs,
        )
        payload = {k: v for k, v in payload.items() if v}
        response = self._post(payload)

        # Check for context_length_exceeded error for single, long input message
        if "error" in response and len(messages) == 1:
            openai_single_message_context_length_exceeded(response["error"])

        message = openai_response_to_message(response)
        usage = self.get_usage(response)
        return message, usage

    @retry_procedure
    def _post(self, payload: dict) -> dict:
        response = self.client.post("chat/completions", json=payload)
        return raise_for_status(response).json()
