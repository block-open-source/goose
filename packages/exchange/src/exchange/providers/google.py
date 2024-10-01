import os
from typing import Any, Dict, List, Tuple, Type

import httpx

from exchange import Message, Tool
from exchange.content import Text, ToolResult, ToolUse
from exchange.providers.base import Provider, Usage
from tenacity import retry, wait_fixed, stop_after_attempt
from exchange.providers.utils import retry_if_status
from exchange.providers.utils import raise_for_status

GOOGLE_HOST = "https://generativelanguage.googleapis.com/v1beta"

retry_procedure = retry(
    wait=wait_fixed(2),
    stop=stop_after_attempt(2),
    retry=retry_if_status(codes=[429], above=500),
    reraise=True,
)


class GoogleProvider(Provider):
    def __init__(self, client: httpx.Client) -> None:
        self.client = client

    @classmethod
    def from_env(cls: Type["GoogleProvider"]) -> "GoogleProvider":
        url = os.environ.get("GOOGLE_HOST", GOOGLE_HOST)
        try:
            key = os.environ["GOOGLE_API_KEY"]
        except KeyError:
            raise RuntimeError(
                "Failed to get GOOGLE_API_KEY from the environment, see https://ai.google.dev/gemini-api/docs/api-key"
            )

        client = httpx.Client(
            base_url=url,
            headers={
                "Content-Type": "application/json",
            },
            params={"key": key},
            timeout=httpx.Timeout(60 * 10),
        )
        return cls(client)

    @staticmethod
    def get_usage(data: Dict) -> Usage:  # noqa: ANN401
        usage = data.get("usageMetadata")
        input_tokens = usage.get("promptTokenCount")
        output_tokens = usage.get("candidatesTokenCount")
        total_tokens = usage.get("totalTokenCount")

        if total_tokens is None and input_tokens is not None and output_tokens is not None:
            total_tokens = input_tokens + output_tokens

        return Usage(
            input_tokens=input_tokens,
            output_tokens=output_tokens,
            total_tokens=total_tokens,
        )

    @staticmethod
    def google_response_to_message(response: Dict) -> Message:
        candidates = response.get("candidates", [])
        if candidates:
            # Only use first candidate for now
            candidate = candidates[0]
            content_parts = candidate.get("content", {}).get("parts", [])
            content = []
            for part in content_parts:
                if "text" in part:
                    content.append(Text(text=part["text"]))
                elif "functionCall" in part:
                    content.append(
                        ToolUse(
                            id=part["functionCall"].get("name", ""),
                            name=part["functionCall"].get("name", ""),
                            parameters=part["functionCall"].get("args", {}),
                        )
                    )
            return Message(role="assistant", content=content)

        # If no valid candidates were found, return an empty message
        return Message(role="assistant", content=[])

    @staticmethod
    def tools_to_google_spec(tools: Tuple[Tool]) -> Dict[str, List[Dict[str, Any]]]:
        if not tools:
            return {}
        converted_tools = []
        for tool in tools:
            converted_tool: Dict[str, Any] = {
                "name": tool.name,
                "description": tool.description or "",
            }
            if tool.parameters["properties"]:
                converted_tool["parameters"] = tool.parameters
            converted_tools.append(converted_tool)
        return {"functionDeclarations": converted_tools}

    @staticmethod
    def messages_to_google_spec(messages: List[Message]) -> List[Dict[str, Any]]:
        messages_spec = []
        for message in messages:
            role = "user" if message.role == "user" else "model"
            converted = {"role": role, "parts": []}
            for content in message.content:
                if isinstance(content, Text):
                    converted["parts"].append({"text": content.text})
                elif isinstance(content, ToolUse):
                    converted["parts"].append({"functionCall": {"name": content.name, "args": content.parameters}})
                elif isinstance(content, ToolResult):
                    converted["parts"].append(
                        {"functionResponse": {"name": content.tool_use_id, "response": {"content": content.output}}}
                    )
            messages_spec.append(converted)

        if not messages_spec:
            messages_spec.append({"role": "user", "parts": [{"text": "Ignore"}]})

        return messages_spec

    def complete(
        self,
        model: str,
        system: str,
        messages: List[Message],
        tools: List[Tool] = [],
        **kwargs: Dict[str, Any],
    ) -> Tuple[Message, Usage]:
        tools_set = set()
        unique_tools = []
        for tool in tools:
            if tool.name not in tools_set:
                unique_tools.append(tool)
                tools_set.add(tool.name)

        payload = dict(
            system_instruction={"parts": [{"text": system}]},
            contents=self.messages_to_google_spec(messages),
            tools=self.tools_to_google_spec(tuple(unique_tools)),
            **kwargs,
        )
        payload = {k: v for k, v in payload.items() if v}
        response = self._post(payload, model)
        message = self.google_response_to_message(response)
        usage = self.get_usage(response)
        return message, usage

    @retry_procedure
    def _post(self, payload: dict, model: str) -> httpx.Response:
        response = self.client.post("models/" + model + ":generateContent", json=payload)
        return raise_for_status(response).json()
