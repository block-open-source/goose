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

    @tool
    def ask_reasoner(self, input: str) -> str:
        """
        A general question when asked to think about it.

        Args:
            input (str): A general query

        Returns:
            response (str): the reasoners response
        """
        # Create an instance of Exchange with the inlined OpenAI provider
        self.notifier.log("deep in thought... ")
        provider = self.OpenAiProvider.from_env()
        exchange = Exchange(provider=provider, model="o1-mini", system=None)
        response = ask_an_ai(input=input, exchange=exchange, prompt="please answer the following: " + input)
        return response.content[0].text

    @tool
    def deep_understand(self, code: str, query:str, context: List[dict]) -> str:
        """
        Use this tool if needing to understand and reason about a piece of code based on the context more deeply than previously tried.

        Args:
            code (str): the code to be examined
            query (str): the question being asked
            context (List[dict]): A map of file name to file content of the relevant context

        Returns:
            response (str): the answer about the code
        """
        
        self.notifier.log("analyzing code... ")
        provider = self.OpenAiProvider.from_env()
        exchange = Exchange(provider=provider, model="o1-mini", system=None)
        # Create messages list
        messages = [Message(role="user", content="You are a helpful assistant.")]

        for ctx in context:
            for file_name, file_content in ctx.items():
                messages.append(Message(role="user", content=f"File: {file_name}\n{file_content}"))

        # Append the code to be examined
        messages.append(Message(role="user", content=f"Code: {code}"))

        response = ask_an_ai(input=messages, exchange=exchange, prompt=query)
        return response.content[0].text


    def message_content(self, content: Content) -> Text:
        if isinstance(content, Text):
            return content
        else:
            return Text(str(content))


    @tool
    def deep_debug(self, problem:str) -> str:
        """
        If there have been some iterations and failed to solve an error, try this tool with the problem description.

        Args:
            problem (str): optional description of problem

        Returns:
            response (str): A solution, which may include a suggestion or code snippet.
        """
        # Create an instance of Exchange with the inlined OpenAI provider
        self.notifier.status("thinking on how to solve the problem... ")
        provider = self.OpenAiProvider.from_env()

        # Create messages list
        existing_messages_copy = [
            Message(role=msg.role, content=[self.message_content(content) for content in msg.content])
            for msg in self.exchange_view.processor.messages]
        print('m', existing_messages_copy)
        exchange = Exchange(provider=provider, model="o1-mini", messages=existing_messages_copy, system=None)

        response = ask_an_ai(input=problem, exchange=exchange)
        return response.content[0].text

    # Provide any system instructions for the model
    # This can be generated dynamically, and is run at startup time
    def system(self) -> str:
        return """**These tools are for deeper reasoning and analysis and code generation. The time taken will be longer, and context is needed. This may be when there have been some iterations, the topic is complex, there have been errors in code generation or the user has asked.**"""  # noqa: E501





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
