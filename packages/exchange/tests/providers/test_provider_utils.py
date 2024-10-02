from copy import deepcopy
import json
from unittest.mock import Mock
from attrs import asdict
import httpx
import pytest
from unittest.mock import patch

from exchange.content import Text, ToolResult, ToolUse
from exchange.message import Message
from exchange.providers.utils import (
    messages_to_openai_spec,
    openai_response_to_message,
    raise_for_status,
    tools_to_openai_spec,
)
from exchange.tool import Tool

OPEN_AI_TOOL_USE_RESPONSE = response = {
    "choices": [
        {
            "role": "assistant",
            "message": {
                "tool_calls": [
                    {
                        "id": "1",
                        "function": {
                            "name": "example_fn",
                            "arguments": json.dumps(
                                {
                                    "param": "value",
                                }
                            ),
                            # TODO: should this handle dict's as well?
                        },
                    }
                ],
            },
        }
    ],
    "usage": {
        "input_tokens": 10,
        "output_tokens": 25,
        "total_tokens": 35,
    },
}


def example_fn(param: str) -> None:
    """
    Testing function.

    Args:
        param (str): Description of param1
    """
    pass


def example_fn_two() -> str:
    """
    Second testing function

    Returns:
        str: Description of return value
    """
    pass


def test_raise_for_status_success() -> None:
    response = Mock(spec=httpx.Response)
    response.status_code = 200

    result = raise_for_status(response)

    assert result == response


def test_raise_for_status_failure_with_text() -> None:
    response = Mock(spec=httpx.Response)
    response.status_code = 404
    response.text = "Not Found: John Cena"

    try:
        raise_for_status(response)
    except httpx.HTTPStatusError as e:
        assert e.response == response
        assert str(e) == "404 Not Found: John Cena"
        assert e.request is None


def test_raise_for_status_failure_without_text() -> None:
    response = Mock(spec=httpx.Response)
    response.status_code = 500
    response.text = ""

    try:
        raise_for_status(response)
    except httpx.HTTPStatusError as e:
        assert e.response == response
        assert str(e) == "500 Internal Server Error"
        assert e.request is None


def test_messages_to_openai_spec() -> None:
    messages = [
        Message(role="assistant", content=[Text("Hello!")]),
        Message(role="user", content=[Text("How are you?")]),
        Message(
            role="assistant",
            content=[ToolUse(id=1, name="tool1", parameters={"param1": "value1"})],
        ),
        Message(role="user", content=[ToolResult(tool_use_id=1, output="Result")]),
    ]

    spec = messages_to_openai_spec(messages)

    assert spec == [
        {"role": "assistant", "content": "Hello!"},
        {"role": "user", "content": "How are you?"},
        {
            "role": "assistant",
            "tool_calls": [
                {
                    "id": 1,
                    "type": "function",
                    "function": {
                        "name": "tool1",
                        "arguments": '{"param1": "value1"}',
                    },
                }
            ],
        },
        {
            "role": "tool",
            "content": "Result",
            "tool_call_id": 1,
        },
    ]


def test_tools_to_openai_spec() -> None:
    tools = (Tool.from_function(example_fn), Tool.from_function(example_fn_two))
    assert len(tools_to_openai_spec(tools)) == 2


def test_tools_to_openai_spec_duplicate() -> None:
    tools = (Tool.from_function(example_fn), Tool.from_function(example_fn))
    with pytest.raises(ValueError):
        tools_to_openai_spec(tools)


def test_tools_to_openai_spec_single() -> None:
    tools = Tool.from_function(example_fn)
    expected_spec = [
        {
            "type": "function",
            "function": {
                "name": "example_fn",
                "description": "Testing function.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "param": {
                            "type": "string",
                            "description": "Description of param1",
                        }
                    },
                    "required": ["param"],
                },
            },
        },
    ]
    result = tools_to_openai_spec((tools,))
    assert result == expected_spec


def test_tools_to_openai_spec_empty() -> None:
    tools = ()
    expected_spec = []
    assert tools_to_openai_spec(tools) == expected_spec


def test_openai_response_to_message_text() -> None:
    response = {
        "choices": [
            {
                "role": "assistant",
                "message": {"content": "Hello from John Cena!"},
            }
        ],
        "usage": {
            "input_tokens": 10,
            "output_tokens": 25,
            "total_tokens": 35,
        },
    }

    message = openai_response_to_message(response)

    actual = asdict(message)
    expect = asdict(
        Message(
            role="assistant",
            content=[Text("Hello from John Cena!")],
        )
    )
    actual.pop("id")
    expect.pop("id")
    assert actual == expect


def test_openai_response_to_message_valid_tooluse() -> None:
    response = deepcopy(OPEN_AI_TOOL_USE_RESPONSE)
    message = openai_response_to_message(response)
    actual = asdict(message)
    expect = asdict(
        Message(
            role="assistant",
            content=[ToolUse(id=1, name="example_fn", parameters={"param": "value"})],
        )
    )
    actual.pop("id")
    actual["content"][0].pop("id")
    expect.pop("id")
    expect["content"][0].pop("id")
    assert actual == expect


def test_openai_response_to_message_invalid_func_name() -> None:
    response = deepcopy(OPEN_AI_TOOL_USE_RESPONSE)
    response["choices"][0]["message"]["tool_calls"][0]["function"]["name"] = "invalid fn"
    message = openai_response_to_message(response)
    assert message.content[0].name == "invalid fn"
    assert json.loads(message.content[0].parameters) == {"param": "value"}
    assert message.content[0].is_error
    assert message.content[0].error_message.startswith("The provided function name")


@patch("json.loads", side_effect=json.JSONDecodeError("error", "doc", 0))
def test_openai_response_to_message_json_decode_error(mock_json) -> None:
    response = deepcopy(OPEN_AI_TOOL_USE_RESPONSE)
    message = openai_response_to_message(response)
    assert message.content[0].name == "example_fn"
    assert message.content[0].is_error
    assert message.content[0].error_message.startswith("Could not interpret tool use")
