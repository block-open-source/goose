import os
import json
import re

import httpx
import base64

from exchange.content import Text, ToolResult, ToolUse
from exchange.message import Message
from exchange.providers.base import Provider, Usage
from exchange.tool import Tool
from tenacity import retry, wait_fixed, stop_after_attempt
from exchange.providers.utils import retry_if_status
from exchange.langfuse_wrapper import observe_wrapper
# Copied local functions for openai_response_to_message modification.

def openai_response_to_message(response: dict) -> Message:
    original = response["choices"][0]["message"]
    content = []
    text = original.get("content")
    if text:
        content.append(Text(text=text))

    tool_calls = original.get("tool_calls")
    if tool_calls:
        for tool_call in tool_calls:
            try:
                function_name = tool_call["function"]["name"]
                if not re.match(r"^[a-zA-Z0-9_-]+$", function_name):
                    content.append(
                        ToolUse(
                            id=tool_call["id"],
                            name=function_name,
                            parameters=tool_call["function"]["arguments"],
                            is_error=True,
                            error_message=f"The provided function name '{function_name}' had invalid characters, it must match this regex [a-zA-Z0-9_-]+",
                        )
                    )
                else:
                    content.append(
                        ToolUse(
                            id=tool_call["id"],
                            name=function_name,
                            parameters=json.loads(tool_call["function"]["arguments"]),
                        )
                    )
            except json.JSONDecodeError:
                content.append(
                    ToolUse(
                        id=tool_call["id"],
                        name=tool_call["function"]["name"],
                        parameters=tool_call["function"]["arguments"],
                        is_error=True,
                        error_message=f"Could not interpret tool use parameters for id {tool_call['id']}: {tool_call['function']['arguments']}",
                    )
                )

    return Message(role="assistant", content=content)


from exchange.providers.utils import ( openai_single_message_context_length_exceeded, raise_for_status )

# Copied local functions for modification


def encode_image(image_path: str) -> str:
    with open(image_path, "rb") as image_file:
        return base64.b64encode(image_file.read()).decode("utf-8")

def messages_to_openai_spec(messages: list[Message]) -> list[dict[str, any]]:
    messages_spec = []
    for message in messages:
        converted = {"role": message.role}
        output = []
        for content in message.content:
            if isinstance(content, Text):
                converted["content"] = content.text
            elif isinstance(content, ToolUse):
                sanitized_name = re.sub(r"[^a-zA-Z0-9_-]", "_", content.name)
                converted.setdefault("tool_calls", []).append(
                    {
                        "id": content.id,
                        "type": "function",
                        "function": {
                            "name": sanitized_name,
                            "arguments": json.dumps(content.parameters),
                        },
                    }
                )
            elif isinstance(content, ToolResult):
                if content.output.startswith('"image:'):
                    image_path = content.output.replace('"image:', "").replace('"', "")
                    output.append(
                        {
                            "role": "tool",
                            "content": [
                                {
                                    "type": "text",
                                    "text": "This tool result included an image that is uploaded in the next message.",
                                },
                            ],
                            "tool_call_id": content.tool_use_id,
                        }
                    )
                    output.append(
                        {
                            "role": "user",
                            "content": [
                                {
                                    "type": "image_url",
                                    "image_url": {"url": f"data:image/jpeg;base64,{encode_image(image_path)}"},
                                }
                            ],
                        }
                    )

                else:
                    output.append(
                        {
                            "role": "tool",
                            "content": content.output,
                            "tool_call_id": content.tool_use_id,
                        }
                    )

        if "content" in converted or "tool_calls" in converted:
            output = [converted] + output
        messages_spec.extend(output)
    return messages_spec


def tools_to_openai_spec(tools: tuple[Tool, ...]) -> dict[str, any]:
    tools_names = set()
    result = []
    for tool in tools:
        if tool.name in tools_names:
            # we should never allow duplicate tools
            raise ValueError(f"Duplicate tool name: {tool.name}")
        result.append(
            {
                "type": "function",
                "function": {
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.parameters,
                },
            }
        )
        tools_names.add(tool.name)
    return result





OPENAI_HOST = "https://api.openai.com/"

retry_procedure = retry(
    wait=wait_fixed(2),
    stop=stop_after_attempt(2),
    retry=retry_if_status(codes=[429], above=500),
    reraise=True,
)


class OpenAiRealtimeProvider(Provider):
    """Provides chat completions for models hosted directly by OpenAI."""

    PROVIDER_NAME = "openai"
    REQUIRED_ENV_VARS = ["OPENAI_API_KEY"]
    instructions_url = "https://platform.openai.com/docs/api-reference/api-keys"

    def __init__(self, client: httpx.Client) -> None:
        self.client = client

    @classmethod
    def from_env(cls: type["OpenAiRealtimeProvider"]) -> "OpenAiRealtimeProvider":
        cls.check_env_vars(cls.instructions_url)
        url = os.environ.get("OPENAI_HOST", OPENAI_HOST)
        key = os.environ.get("OPENAI_API_KEY")

        client = httpx.Client(
            base_url=url + "v1/",
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

    @observe_wrapper(as_type="generation")
    def complete(
        self,
        model: str,
        system: str,
        messages: list[Message],
        tools: tuple[Tool, ...],
        **kwargs: dict[str, any],
    ) -> tuple[Message, Usage]:
        system_message = [] if model.startswith("o1") else [{"role": "system", "content": system}]
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
        # Note: While OpenAI and Ollama mount the API under "v1", this is
        # conventional and not a strict requirement. For example, Azure OpenAI
        # mounts the API under the deployment name, and "v1" is not in the URL.
        # See https://github.com/openai/openai-openapi/blob/master/openapi.yaml
        response = self.client.post("chat/completions", json=payload)
        return raise_for_status(response).json()

