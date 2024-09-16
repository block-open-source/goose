import re
import traceback
import json
from exchange import Exchange
from exchange.content import Text
from exchange.message import Message
from exchange.tool import Tool
from typing import Optional

MAX_RETRY = 4


def decode(response, schema):
    match = re.search("```json\n(.*)\n```", response, re.DOTALL)
    if not match:
        raise ValueError("Could not find a json formatted code block")
    blob = match.group(1).strip()
    try:
        content = json.loads(blob)
    except json.JSONDecodeError:
        raise ValueError(f"{blob} could not be parsed as json")
    for key in schema["required"]:
        if key not in content:
            raise ValueError(f"{key} is a required field")

    for key in content:
        if key not in schema["properties"]:
            raise ValueError(f"{key} is not a known field")
    return content


def pseudotool(exchange: Exchange, tool: Tool, hints: Optional[str] = None):
    """Request a response from text generation that conforms to model

    This approximates tool usage, with no ability to select a tool but
    instead to cast the result to the schema.

    The exchange is expected to have all required kickoff messages, and
    forwarding information will be appended to it.
    """
    if exchange.messages[-1].role != "user":
        raise ValueError(
            "Exchange history does not end with a user message to request tool use"
        )

    if hints:
        first = None
        for content in exchange.messages[0].content:
            if isinstance(content, Text):
                first = content
        first.text += "\n\nHints:\n{hints}"

    final = None
    for content in exchange.messages[-1].content:
        if isinstance(content, Text):
            final = content
    if not final:
        raise ValueError(
            "Exchange history does not include a user message to request tool use"
        )

    final.text += Message.load("psuedotool.md", tool=tool).text

    arguments = None
    n_retry = 0
    while n_retry < MAX_RETRY:
        response = exchange.generate()
        try:
            arguments = decode(response.text, tool.parameters)
        except ValueError as e:
            exchange.add(
                Message.user(
                    f"We failed to read the json payload from your response.\nIt caused the following error:\n\n{e}\n\nPlease reply with a valid json block for the above schema."
                )
            )
            continue
        try:
            return tool.function(**arguments)
        except Exception as e:
            tb = traceback.format_exc()
            exchange.add(
                Message.user(
                    f"The tool call you provided caused an error: \n\n{tb}\n\nYou can make another attempt to fix this issue."
                )
            )
        n_retry += 1
    raise RuntimeError(f"Attempt to run {tool.name} failed with {MAX_RETRY} attempts.")
