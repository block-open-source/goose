import os
from typing import Any, Dict, List, Tuple, Type
import httpx
from exchange import Exchange, Message, Text
from exchange.content import Content
from exchange.providers import Provider, Usage
from exchange.tool import Tool
from exchange.providers.retry_with_back_off_decorator import retry_httpx_request
from exchange.providers.utils import raise_for_status, openai_single_message_context_length_exceeded
from exchange.providers.utils import openai_response_to_message, messages_to_openai_spec
from goose.toolkit.base import Toolkit, tool
from goose.utils.ask import ask_an_ai


class Reasoner(Toolkit):
    """This is a toolkit to add deeper and slower reasoning around code and questions and debugging"""

    def message_content(self, content: Content) -> Text:
        if isinstance(content, Text):
            return content
        else:
            return Text(str(content))

    @tool
    def deep_debug(self, problem: str) -> str:
        """
        This tool can assist with debugging when there are errors or problems when trying things
        and other approaches haven't solved it.
        It will take a minute to think about it and consider solutions.

        Args:
            problem (str): description of problem or errors seen.

        Returns:
            response (str): A solution, which may include a suggestion or code snippet.
        """
        # Create an instance of Exchange with the inlined OpenAI provider
        self.notifier.status("thinking...")
        provider = self.OpenAiProvider.from_env()

        # Create messages list
        existing_messages_copy = [
            Message(role=msg.role, content=[self.message_content(content) for content in msg.content])
            for msg in self.exchange_view.processor.messages
        ]
        exchange = Exchange(provider=provider, model="o1-preview", messages=existing_messages_copy, system=None)

        response = ask_an_ai(input="Can you help debug this problem: " + problem, exchange=exchange)
        return response.content[0].text

    @tool
    def generate_code(self, instructions: str) -> str:
        """
        Use this when enhancing existing code or generating new code unless it is simple or
        it is required quickly.
        This can generate high quality code to be considered and used

        Args:
            instructions (str): instructions of what code to write or how to modify it.

        Returns:
            response (str): generated code to be tested or applied as needed.
        """
        # Create an instance of Exchange with the inlined OpenAI provider
        self.notifier.status("generating code...")
        provider = self.OpenAiProvider.from_env()

        # clone messages, converting to text for context
        existing_messages_copy = [
            Message(role=msg.role, content=[self.message_content(content) for content in msg.content])
            for msg in self.exchange_view.processor.messages
        ]
        exchange = Exchange(provider=provider, model="o1-mini", messages=existing_messages_copy, system=None)

        response = ask_an_ai(input=instructions, exchange=exchange)
        return response.content[0].text

    def system(self) -> str:
        """Retrieve instructions on how to use this reasoning and code generation tool"""
        return Message.load("prompts/reasoner.jinja").text

    class OpenAiProvider(Provider):
        """Inlined here as o1 model only in preview and supports very little still."""

        def __init__(self, client: httpx.Client) -> None:
            super().__init__()
            self.client = client

        @classmethod
        def from_env(cls: Type["Reasoner.OpenAiProvider"]) -> "Reasoner.OpenAiProvider":
            url = os.environ.get("OPENAI_HOST", "https://api.openai.com/")
            try:
                key = os.environ["OPENAI_API_KEY"]
            except KeyError:
                raise RuntimeError(
                    "Failed to get OPENAI_API_KEY from the environment, see https://platform.openai.com/docs/api-reference/api-keys"
                )
            client = httpx.Client(
                base_url=url,
                auth=("Bearer", key),
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
                    *messages_to_openai_spec(messages),
                ],
                model=model,
                **kwargs,
            )
            payload = {k: v for k, v in payload.items() if v}
            response = self._send_request(payload)

            # Check for context_length_exceeded error for single, long input message
            if "error" in response.json() and len(messages) == 1:
                openai_single_message_context_length_exceeded(response.json()["error"])

            data = raise_for_status(response).json()

            message = openai_response_to_message(data)
            usage = self.get_usage(data)
            return message, usage

        @retry_httpx_request()
        def _send_request(self, payload: Any) -> httpx.Response:  # noqa: ANN401
            return self.client.post("v1/chat/completions", json=payload)
