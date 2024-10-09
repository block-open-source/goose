from typing import List, Tuple

import pytest

from exchange.checkpoint import Checkpoint, CheckpointData
from exchange.content import Text, ToolResult, ToolUse
from exchange.exchange import Exchange
from exchange.message import Message
from exchange.moderators import PassiveModerator
from exchange.providers import Provider, Usage
from exchange.tool import Tool


def dummy_tool() -> str:
    """An example tool"""
    return "dummy response"


too_long_output = "x" * (2**20 + 1)
too_long_token_output = "word " * 128000


def no_overlapping_checkpoints(exchange: Exchange) -> bool:
    """Assert that there are no overlapping checkpoints in the exchange."""
    for i, checkpoint in enumerate(exchange.checkpoint_data.checkpoints):
        for other_checkpoint in exchange.checkpoint_data.checkpoints[i + 1 :]:
            if not checkpoint.end_index < other_checkpoint.start_index:
                return False
    return True


def checkpoint_to_index_pairs(checkpoints: List[Checkpoint]) -> List[Tuple[int, int]]:
    return [(checkpoint.start_index, checkpoint.end_index) for checkpoint in checkpoints]


class MockProvider(Provider):
    def __init__(self, sequence: List[Message], usage_dicts: List[dict]):
        # We'll use init to provide a preplanned reply sequence
        self.sequence = sequence
        self.call_count = 0
        self.usage_dicts = usage_dicts

    @staticmethod
    def get_usage(data: dict) -> Usage:
        usage = data.pop("usage")
        input_tokens = usage.get("input_tokens")
        output_tokens = usage.get("output_tokens")
        total_tokens = usage.get("total_tokens")

        if total_tokens is None and input_tokens is not None and output_tokens is not None:
            total_tokens = input_tokens + output_tokens

        return Usage(
            input_tokens=input_tokens,
            output_tokens=output_tokens,
            total_tokens=total_tokens,
        )

    def complete(self, model: str, system: str, messages: List[Message], tools: List[Tool]) -> Message:
        output = self.sequence[self.call_count]
        usage = self.get_usage(self.usage_dicts[self.call_count])
        self.call_count += 1
        return (output, usage)


def test_reply_with_unsupported_tool():
    ex = Exchange(
        provider=MockProvider(
            sequence=[
                Message(
                    role="assistant",
                    content=[ToolUse(id="1", name="unsupported_tool", parameters={})],
                ),
                Message(
                    role="assistant",
                    content=[Text(text="Here is the completion after tool call")],
                ),
            ],
            usage_dicts=[
                {"usage": {"input_tokens": 12, "output_tokens": 23}},
                {"usage": {"input_tokens": 12, "output_tokens": 23}},
            ],
        ),
        model="gpt-4o-2024-05-13",
        system="You are a helpful assistant.",
        tools=(Tool.from_function(dummy_tool),),
        moderator=PassiveModerator(),
    )

    ex.add(Message(role="user", content=[Text(text="test")]))

    ex.reply()

    content = ex.messages[-2].content[0]
    assert isinstance(content, ToolResult) and content.is_error and "no tool exists" in content.output.lower()


def test_invalid_tool_parameters():
    """Test handling of invalid tool parameters response"""
    ex = Exchange(
        provider=MockProvider(
            sequence=[
                Message(
                    role="assistant",
                    content=[ToolUse(id="1", name="dummy_tool", parameters="invalid json")],
                ),
                Message(
                    role="assistant",
                    content=[Text(text="Here is the completion after tool call")],
                ),
            ],
            usage_dicts=[
                {"usage": {"input_tokens": 12, "output_tokens": 23}},
                {"usage": {"input_tokens": 12, "output_tokens": 23}},
            ],
        ),
        model="gpt-4o-2024-05-13",
        system="You are a helpful assistant.",
        tools=[Tool.from_function(dummy_tool)],
        moderator=PassiveModerator(),
    )

    ex.add(Message(role="user", content=[Text(text="test invalid parameters")]))

    ex.reply()

    content = ex.messages[-2].content[0]
    assert isinstance(content, ToolResult) and content.is_error and "invalid json" in content.output.lower()


def test_max_tool_use_when_limit_reached():
    """Test the max_tool_use parameter in the reply method."""
    ex = Exchange(
        provider=MockProvider(
            sequence=[
                Message(
                    role="assistant",
                    content=[ToolUse(id="1", name="dummy_tool", parameters={})],
                ),
                Message(
                    role="assistant",
                    content=[ToolUse(id="2", name="dummy_tool", parameters={})],
                ),
                Message(
                    role="assistant",
                    content=[ToolUse(id="3", name="dummy_tool", parameters={})],
                ),
            ],
            usage_dicts=[
                {"usage": {"input_tokens": 12, "output_tokens": 23}},
                {"usage": {"input_tokens": 12, "output_tokens": 23}},
                {"usage": {"input_tokens": 12, "output_tokens": 23}},
            ],
        ),
        model="gpt-4o-2024-05-13",
        system="You are a helpful assistant.",
        tools=[Tool.from_function(dummy_tool)],
        moderator=PassiveModerator(),
    )

    ex.add(Message(role="user", content=[Text(text="test max tool use")]))

    response = ex.reply(max_tool_use=3)

    assert ex.provider.call_count == 3
    assert "reached the limit" in response.content[0].text.lower()

    assert isinstance(ex.messages[-2].content[0], ToolResult) and ex.messages[-2].content[0].tool_use_id == "3"

    assert ex.messages[-1].role == "assistant"


def test_tool_output_too_long_character_error():
    """Test tool handling when output exceeds character limit."""

    def long_output_tool_char() -> str:
        return too_long_output

    ex = Exchange(
        provider=MockProvider(
            sequence=[
                Message(
                    role="assistant",
                    content=[ToolUse(id="1", name="long_output_tool_char", parameters={})],
                ),
                Message(
                    role="assistant",
                    content=[Text(text="Here is the completion after tool call")],
                ),
            ],
            usage_dicts=[
                {"usage": {"input_tokens": 12, "output_tokens": 23}},
                {"usage": {"input_tokens": 12, "output_tokens": 23}},
            ],
        ),
        model="gpt-4o-2024-05-13",
        system="You are a helpful assistant.",
        tools=[Tool.from_function(long_output_tool_char)],
        moderator=PassiveModerator(),
    )

    ex.add(Message(role="user", content=[Text(text="test long output char")]))

    ex.reply()

    content = ex.messages[-2].content[0]
    assert (
        isinstance(content, ToolResult)
        and content.is_error
        and "output that was too long to handle" in content.output.lower()
    )


def test_tool_output_too_long_token_error():
    """Test tool handling when output exceeds token limit."""

    def long_output_tool_token() -> str:
        return too_long_token_output

    ex = Exchange(
        provider=MockProvider(
            sequence=[
                Message(
                    role="assistant",
                    content=[ToolUse(id="1", name="long_output_tool_token", parameters={})],
                ),
                Message(
                    role="assistant",
                    content=[Text(text="Here is the completion after tool call")],
                ),
            ],
            usage_dicts=[
                {"usage": {"input_tokens": 12, "output_tokens": 23}},
                {"usage": {"input_tokens": 12, "output_tokens": 23}},
            ],
        ),
        model="gpt-4o-2024-05-13",
        system="You are a helpful assistant.",
        tools=[Tool.from_function(long_output_tool_token)],
        moderator=PassiveModerator(),
    )

    ex.add(Message(role="user", content=[Text(text="test long output token")]))

    ex.reply()

    content = ex.messages[-2].content[0]
    assert (
        isinstance(content, ToolResult)
        and content.is_error
        and "output that was too long to handle" in content.output.lower()
    )


@pytest.fixture(scope="function")
def normal_exchange() -> Exchange:
    ex = Exchange(
        provider=MockProvider(
            sequence=[
                Message(role="assistant", content=[Text(text="Message 1")]),
                Message(role="assistant", content=[Text(text="Message 2")]),
                Message(role="assistant", content=[Text(text="Message 3")]),
                Message(role="assistant", content=[Text(text="Message 4")]),
                Message(role="assistant", content=[Text(text="Message 5")]),
            ],
            usage_dicts=[
                {"usage": {"total_tokens": 10, "input_tokens": 5, "output_tokens": 5}},
                {"usage": {"total_tokens": 28, "input_tokens": 10, "output_tokens": 18}},
                {"usage": {"total_tokens": 33, "input_tokens": 28, "output_tokens": 5}},
                {"usage": {"total_tokens": 40, "input_tokens": 32, "output_tokens": 8}},
                {"usage": {"total_tokens": 50, "input_tokens": 40, "output_tokens": 10}},
            ],
        ),
        model="gpt-4o-2024-05-13",
        system="You are a helpful assistant.",
        tools=(Tool.from_function(dummy_tool),),
        moderator=PassiveModerator(),
        checkpoint_data=CheckpointData(),
    )
    return ex


@pytest.fixture(scope="function")
def resumed_exchange() -> Exchange:
    messages = [
        Message(role="user", content=[Text(text="User message 1")]),
        Message(role="assistant", content=[Text(text="Assistant Message 1")]),
        Message(role="user", content=[Text(text="User message 2")]),
        Message(role="assistant", content=[Text(text="Assistant Message 2")]),
        Message(role="user", content=[Text(text="User message 3")]),
        Message(role="assistant", content=[Text(text="Assistant Message 3")]),
    ]
    provider = MockProvider(
        sequence=[
            Message(role="assistant", content=[Text(text="Assistant Message 4")]),
        ],
        usage_dicts=[
            {"usage": {"total_tokens": 40, "input_tokens": 32, "output_tokens": 8}},
        ],
    )
    ex = Exchange(
        provider=provider,
        messages=messages,
        tools=[],
        model="gpt-4o-2024-05-13",
        system="You are a helpful assistant.",
        checkpoint_data=CheckpointData(),
        moderator=PassiveModerator(),
    )
    return ex


def test_checkpoints_on_exchange(normal_exchange):
    """Test checkpoints on an exchange."""
    ex = normal_exchange
    ex.add(Message(role="user", content=[Text(text="User message")]))
    ex.reply()
    ex.add(Message(role="user", content=[Text(text="User message")]))
    ex.reply()
    ex.add(Message(role="user", content=[Text(text="User message")]))
    ex.reply()

    # Check if checkpoints are created correctly
    checkpoints = ex.checkpoint_data.checkpoints
    assert len(checkpoints) == 6
    for i in range(len(ex.messages)):
        # asserting that each message has a corresponding checkpoint
        assert checkpoints[i].start_index == i
        assert checkpoints[i].end_index == i

    # Check if the messages are ordered correctly
    assert [msg.content[0].text for msg in ex.messages] == [
        "User message",
        "Message 1",
        "User message",
        "Message 2",
        "User message",
        "Message 3",
    ]
    assert no_overlapping_checkpoints(ex)


def test_checkpoints_on_resumed_exchange(resumed_exchange) -> None:
    ex = resumed_exchange
    ex.pop_last_message()
    ex.reply()

    checkpoints = ex.checkpoint_data.checkpoints
    assert len(checkpoints) == 2
    assert len(ex.messages) == 6
    assert checkpoints[0].token_count == 32
    assert checkpoints[0].start_index == 0
    assert checkpoints[0].end_index == 4
    assert checkpoints[1].token_count == 8
    assert checkpoints[1].start_index == 5
    assert checkpoints[1].end_index == 5
    assert no_overlapping_checkpoints(ex)


def test_pop_last_checkpoint_on_resumed_exchange(resumed_exchange) -> None:
    ex = resumed_exchange
    ex.add(Message(role="user", content=[Text(text="Assistant Message 4")]))
    ex.reply()
    ex.pop_last_checkpoint()

    assert len(ex.messages) == 7
    assert len(ex.checkpoint_data.checkpoints) == 1

    ex.pop_last_checkpoint()
    assert len(ex.messages) == 0
    assert len(ex.checkpoint_data.checkpoints) == 0
    assert no_overlapping_checkpoints(ex)


def test_pop_last_checkpoint_on_normal_exchange(normal_exchange) -> None:
    ex = normal_exchange
    ex.add(Message(role="user", content=[Text(text="User message")]))
    ex.reply()
    ex.add(Message(role="user", content=[Text(text="User message")]))
    ex.reply()
    ex.pop_last_checkpoint()
    ex.pop_last_checkpoint()

    assert len(ex.messages) == 2
    assert len(ex.checkpoint_data.checkpoints) == 2
    assert no_overlapping_checkpoints(ex)
    ex.add(Message(role="user", content=[Text(text="User message")]))
    ex.pop_last_checkpoint()
    assert len(ex.messages) == 1
    assert len(ex.checkpoint_data.checkpoints) == 1
    ex.reply()
    assert len(ex.messages) == 2
    assert len(ex.checkpoint_data.checkpoints) == 2
    assert no_overlapping_checkpoints(ex)


def test_pop_first_message_no_messages():
    ex = Exchange(
        provider=MockProvider(sequence=[], usage_dicts=[]),
        model="gpt-4o-2024-05-13",
        system="You are a helpful assistant.",
        tools=[Tool.from_function(dummy_tool)],
        moderator=PassiveModerator(),
    )

    with pytest.raises(ValueError) as e:
        ex.pop_first_message()
    assert str(e.value) == "There are no messages to pop"


def test_pop_first_message_checkpoint_with_many_messages(resumed_exchange):
    ex = resumed_exchange
    ex.pop_last_message()
    ex.reply()

    assert len(ex.messages) == 6
    assert len(ex.checkpoint_data.checkpoints) == 2
    assert ex.checkpoint_data.checkpoints[0].start_index == 0
    assert ex.checkpoint_data.checkpoints[0].end_index == 4
    assert ex.checkpoint_data.checkpoints[1].start_index == 5
    assert ex.checkpoint_data.checkpoints[1].end_index == 5
    assert ex.checkpoint_data.message_index_offset == 0
    assert no_overlapping_checkpoints(ex)

    ex.pop_first_message()

    assert len(ex.messages) == 5
    assert len(ex.checkpoint_data.checkpoints) == 1
    assert ex.checkpoint_data.checkpoints[0].start_index == 5
    assert ex.checkpoint_data.checkpoints[0].end_index == 5
    assert ex.checkpoint_data.message_index_offset == 1
    assert no_overlapping_checkpoints(ex)

    ex.pop_first_message()

    assert len(ex.messages) == 4
    assert len(ex.checkpoint_data.checkpoints) == 1
    assert ex.checkpoint_data.checkpoints[0].start_index == 5
    assert ex.checkpoint_data.checkpoints[0].end_index == 5
    assert ex.checkpoint_data.message_index_offset == 2
    assert no_overlapping_checkpoints(ex)

    ex.pop_first_message()

    assert len(ex.messages) == 3
    assert len(ex.checkpoint_data.checkpoints) == 1
    assert ex.checkpoint_data.checkpoints[0].start_index == 5
    assert ex.checkpoint_data.checkpoints[0].end_index == 5
    assert ex.checkpoint_data.message_index_offset == 3
    assert no_overlapping_checkpoints(ex)

    ex.pop_first_message()

    assert len(ex.messages) == 2
    assert len(ex.checkpoint_data.checkpoints) == 1
    assert ex.checkpoint_data.checkpoints[0].start_index == 5
    assert ex.checkpoint_data.checkpoints[0].end_index == 5
    assert ex.checkpoint_data.message_index_offset == 4
    assert no_overlapping_checkpoints(ex)

    ex.pop_first_message()

    assert len(ex.messages) == 1
    assert len(ex.checkpoint_data.checkpoints) == 1
    assert ex.checkpoint_data.checkpoints[0].start_index == 5
    assert ex.checkpoint_data.checkpoints[0].end_index == 5
    assert ex.checkpoint_data.message_index_offset == 5
    assert no_overlapping_checkpoints(ex)

    ex.pop_first_message()

    assert len(ex.messages) == 0
    assert len(ex.checkpoint_data.checkpoints) == 0
    assert ex.checkpoint_data.message_index_offset == 0
    assert no_overlapping_checkpoints(ex)

    with pytest.raises(ValueError) as e:
        ex.pop_first_message()

    assert str(e.value) == "There are no messages to pop"


def test_varied_message_manipulation(normal_exchange):
    ex = normal_exchange
    ex.add(Message(role="user", content=[Text(text="User message 1")]))
    ex.reply()

    ex.pop_first_message()

    ex.add(Message(role="user", content=[Text(text="User message 2")]))
    ex.reply()

    assert len(ex.messages) == 3
    assert len(ex.checkpoint_data.checkpoints) == 3
    assert ex.checkpoint_data.message_index_offset == 1
    # (start, end)
    # (1, 1), (2, 2), (3, 3)
    # actual_index_in_messages_arr = any checkpoint index - offset
    assert no_overlapping_checkpoints(ex)
    for i in range(3):
        assert ex.checkpoint_data.checkpoints[i].start_index == i + 1
        assert ex.checkpoint_data.checkpoints[i].end_index == i + 1

    ex.pop_last_message()

    assert len(ex.messages) == 2
    assert len(ex.checkpoint_data.checkpoints) == 2
    assert ex.checkpoint_data.message_index_offset == 1
    assert no_overlapping_checkpoints(ex)
    for i in range(2):
        assert ex.checkpoint_data.checkpoints[i].start_index == i + 1
        assert ex.checkpoint_data.checkpoints[i].end_index == i + 1

    ex.add(Message(role="assistant", content=[Text(text="Assistant message")]))
    ex.add(Message(role="user", content=[Text(text="User message 3")]))
    ex.reply()

    assert len(ex.messages) == 5
    assert len(ex.checkpoint_data.checkpoints) == 4
    assert ex.checkpoint_data.message_index_offset == 1
    assert no_overlapping_checkpoints(ex)
    assert checkpoint_to_index_pairs(ex.checkpoint_data.checkpoints) == [(1, 1), (2, 2), (3, 4), (5, 5)]

    ex.pop_last_checkpoint()

    assert len(ex.messages) == 4
    assert len(ex.checkpoint_data.checkpoints) == 3
    assert ex.checkpoint_data.message_index_offset == 1
    assert no_overlapping_checkpoints(ex)
    assert checkpoint_to_index_pairs(ex.checkpoint_data.checkpoints) == [(1, 1), (2, 2), (3, 4)]

    ex.pop_first_message()

    assert len(ex.messages) == 3
    assert len(ex.checkpoint_data.checkpoints) == 2
    assert ex.checkpoint_data.message_index_offset == 2
    assert no_overlapping_checkpoints(ex)
    assert checkpoint_to_index_pairs(ex.checkpoint_data.checkpoints) == [(2, 2), (3, 4)]

    ex.pop_last_message()

    assert len(ex.messages) == 2
    assert len(ex.checkpoint_data.checkpoints) == 1
    assert ex.checkpoint_data.message_index_offset == 2
    assert no_overlapping_checkpoints(ex)
    assert checkpoint_to_index_pairs(ex.checkpoint_data.checkpoints) == [(2, 2)]

    ex.pop_last_message()
    assert len(ex.messages) == 1
    assert len(ex.checkpoint_data.checkpoints) == 1
    assert ex.checkpoint_data.message_index_offset == 2
    assert no_overlapping_checkpoints(ex)
    assert checkpoint_to_index_pairs(ex.checkpoint_data.checkpoints) == [(2, 2)]

    ex.add(Message(role="assistant", content=[Text(text="Assistant message")]))
    ex.add(Message(role="user", content=[Text(text="User message 5")]))
    ex.pop_last_checkpoint()

    assert len(ex.messages) == 0
    assert len(ex.checkpoint_data.checkpoints) == 0

    ex.add(Message(role="user", content=[Text(text="User message 6")]))
    ex.reply()

    assert len(ex.messages) == 2
    assert len(ex.checkpoint_data.checkpoints) == 2
    assert ex.checkpoint_data.message_index_offset == 2
    assert no_overlapping_checkpoints(ex)
    assert checkpoint_to_index_pairs(ex.checkpoint_data.checkpoints) == [(2, 2), (3, 3)]

    ex.pop_last_message()

    assert len(ex.messages) == 1
    assert len(ex.checkpoint_data.checkpoints) == 1
    assert ex.checkpoint_data.message_index_offset == 2
    assert no_overlapping_checkpoints(ex)
    assert checkpoint_to_index_pairs(ex.checkpoint_data.checkpoints) == [(2, 2)]

    ex.pop_first_message()

    assert len(ex.messages) == 0
    assert len(ex.checkpoint_data.checkpoints) == 0
    assert ex.checkpoint_data.message_index_offset == 0

    ex.add(Message(role="user", content=[Text(text="User message 7")]))
    ex.pop_last_message()

    assert len(ex.messages) == 0
    assert len(ex.checkpoint_data.checkpoints) == 0
    assert ex.checkpoint_data.message_index_offset == 0


def test_pop_last_message_when_no_checkpoints_but_messages_present(normal_exchange):
    ex = normal_exchange
    ex.add(Message(role="user", content=[Text(text="User message")]))

    ex.pop_last_message()

    assert len(ex.messages) == 0
    assert len(ex.checkpoint_data.checkpoints) == 0
    assert ex.checkpoint_data.message_index_offset == 0


def test_pop_first_message_when_no_checkpoints_but_message_present(normal_exchange):
    ex = normal_exchange
    ex.add(Message(role="user", content=[Text(text="User message")]))

    with pytest.raises(ValueError) as e:
        ex.pop_first_message()

    assert str(e.value) == "There must be at least one checkpoint to pop the first message"


def test_pop_first_checkpoint_size_n(resumed_exchange):
    ex = resumed_exchange
    ex.pop_last_message()  # needed because the last message is an assistant message
    ex.reply()

    ex.pop_first_checkpoint()
    assert ex.checkpoint_data.message_index_offset == 5
    assert len(ex.checkpoint_data.checkpoints) == 1
    assert len(ex.messages) == 1

    ex.pop_first_checkpoint()
    assert ex.checkpoint_data.message_index_offset == 0
    assert len(ex.checkpoint_data.checkpoints) == 0
    assert len(ex.messages) == 0


def test_pop_first_checkpoint_size_1(normal_exchange):
    ex = normal_exchange
    ex.add(Message(role="user", content=[Text(text="User message")]))
    ex.reply()

    ex.pop_first_checkpoint()
    assert ex.checkpoint_data.message_index_offset == 1
    assert len(ex.checkpoint_data.checkpoints) == 1
    assert len(ex.messages) == 1

    ex.pop_first_checkpoint()
    assert ex.checkpoint_data.message_index_offset == 0
    assert len(ex.checkpoint_data.checkpoints) == 0
    assert len(ex.messages) == 0


def test_pop_first_checkpoint_no_checkpoints(normal_exchange):
    ex = normal_exchange

    with pytest.raises(ValueError) as e:
        ex.pop_first_checkpoint()

    assert str(e.value) == "There are no checkpoints to pop"


def test_prepend_checkpointed_message_empty_exchange(normal_exchange):
    ex = normal_exchange
    ex.prepend_checkpointed_message(Message(role="assistant", content=[Text(text="Assistant message")]), 10)

    assert ex.checkpoint_data.message_index_offset == 0
    assert len(ex.checkpoint_data.checkpoints) == 1
    assert ex.checkpoint_data.checkpoints[0].start_index == 0
    assert ex.checkpoint_data.checkpoints[0].end_index == 0

    ex.add(Message(role="user", content=[Text(text="User message")]))
    ex.reply()

    assert ex.checkpoint_data.message_index_offset == 0
    assert len(ex.checkpoint_data.checkpoints) == 3
    assert len(ex.messages) == 3
    assert no_overlapping_checkpoints(ex)

    ex.pop_first_checkpoint()

    assert ex.checkpoint_data.message_index_offset == 1
    assert len(ex.checkpoint_data.checkpoints) == 2
    assert len(ex.messages) == 2
    assert no_overlapping_checkpoints(ex)

    ex.prepend_checkpointed_message(Message(role="assistant", content=[Text(text="Assistant message")]), 10)
    assert ex.checkpoint_data.message_index_offset == 0
    assert len(ex.checkpoint_data.checkpoints) == 3
    assert len(ex.messages) == 3
    assert no_overlapping_checkpoints(ex)


def test_generate_successful_response_on_first_try(normal_exchange):
    ex = normal_exchange
    ex.add(Message(role="user", content=[Text("Hello")]))
    ex.generate()


def test_rewind_in_normal_exchange(normal_exchange):
    ex = normal_exchange
    ex.rewind()

    assert len(ex.messages) == 0
    assert len(ex.checkpoint_data.checkpoints) == 0

    ex.add(Message(role="user", content=[Text("Hello")]))
    ex.generate()
    ex.add(Message(role="user", content=[Text("Hello")]))

    # testing if it works with a user text message at the end
    ex.rewind()

    assert len(ex.messages) == 2
    assert len(ex.checkpoint_data.checkpoints) == 2

    ex.add(Message(role="user", content=[Text("Hello")]))
    ex.generate()

    # testing if it works with a non-user text message at the end
    ex.rewind()

    assert len(ex.messages) == 2
    assert len(ex.checkpoint_data.checkpoints) == 2


def test_rewind_with_tool_usage():
    # simulating a real exchange with tool usage
    ex = Exchange(
        provider=MockProvider(
            sequence=[
                Message.assistant("Hello!"),
                Message(
                    role="assistant",
                    content=[ToolUse(id="1", name="dummy_tool", parameters={})],
                ),
                Message(
                    role="assistant",
                    content=[ToolUse(id="2", name="dummy_tool", parameters={})],
                ),
                Message.assistant("Done!"),
            ],
            usage_dicts=[
                {"usage": {"input_tokens": 12, "output_tokens": 23}},
                {"usage": {"input_tokens": 27, "output_tokens": 44}},
                {"usage": {"input_tokens": 50, "output_tokens": 56}},
                {"usage": {"input_tokens": 60, "output_tokens": 76}},
            ],
        ),
        model="gpt-4o-2024-05-13",
        system="You are a helpful assistant.",
        tools=[Tool.from_function(dummy_tool)],
        moderator=PassiveModerator(),
    )
    ex.add(Message(role="user", content=[Text(text="test")]))
    ex.reply()
    ex.add(Message(role="user", content=[Text(text="kick it off!")]))
    ex.reply()

    # removing the last message to simulate not getting a response
    ex.pop_last_message()

    # calling rewind to last user message
    ex.rewind()

    assert len(ex.messages) == 2
    assert len(ex.checkpoint_data.checkpoints) == 2
    assert no_overlapping_checkpoints(ex)
    assert ex.messages[0].content[0].text == "test"
    assert type(ex.messages[1].content[0]) is Text
    assert ex.messages[1].role == "assistant"
