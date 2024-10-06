import pytest
from exchange import Exchange, Message
from exchange.content import ToolResult, ToolUse
from exchange.moderators.passive import PassiveModerator
from exchange.moderators.summarizer import ContextSummarizer
from exchange.providers import Usage


class MockProvider:
    def complete(self, model, system, messages, tools):
        assistant_message_text = "Summarized content here."
        output_tokens = len(assistant_message_text)
        total_input_tokens = sum(len(msg.text) for msg in messages)
        if not messages or messages[-1].role == "assistant":
            message = Message.user(assistant_message_text)
        else:
            message = Message.assistant(assistant_message_text)
        total_tokens = total_input_tokens + output_tokens
        usage = Usage(
            input_tokens=total_input_tokens,
            output_tokens=output_tokens,
            total_tokens=total_tokens,
        )
        return message, usage


@pytest.fixture
def exchange_instance():
    ex = Exchange(
        provider=MockProvider(),
        model="test-model",
        system="test-system",
        messages=[
            Message.user("Hi, can you help me with my homework?"),
            Message.assistant("Of course! What do you need help with?"),
            Message.user("I need help with math problems."),
            Message.assistant("Sure, I can help with that. Let's get started."),
            Message.user("Can you also help with my science homework?"),
            Message.assistant("Yes, I can help with science too."),
            Message.user("That's great! How about history?"),
            Message.assistant("Of course, I can help with history as well."),
            Message.user("Thanks! You're very helpful."),
            Message.assistant("You're welcome! I'm here to help."),
        ],
        moderator=PassiveModerator(),
    )
    return ex


@pytest.fixture
def summarizer_instance():
    return ContextSummarizer(max_tokens=300)


def test_context_summarizer_rewrite(exchange_instance: Exchange, summarizer_instance: ContextSummarizer):
    # Pre-checks
    assert len(exchange_instance.messages) == 10

    exchange_instance.generate()

    # the exchange instance has a PassiveModerator so the messages are not truncated nor summarized
    assert len(exchange_instance.messages) == 11
    assert len(exchange_instance.checkpoint_data.checkpoints) == 2

    # we now tell the summarizer to summarize the exchange
    summarizer_instance.rewrite(exchange_instance)

    assert exchange_instance.checkpoint_data.total_token_count <= 200
    assert len(exchange_instance.messages) == 2

    # Assert that summarized content is the first message
    first_message = exchange_instance.messages[0]
    assert first_message.role == "user" or first_message.role == "assistant"
    assert any("summarized" in content.text.lower() for content in first_message.content)

    # Ensure roles alternate in the output
    for i in range(1, len(exchange_instance.messages)):
        assert (
            exchange_instance.messages[i - 1].role != exchange_instance.messages[i].role
        ), "Messages must alternate between user and assistant"


MESSAGE_SEQUENCE = [
    Message.user("Hi, can you help me with my homework?"),
    Message.assistant("Of course! What do you need help with?"),
    Message.user("I need help with math problems."),
    Message.assistant("Sure, I can help with that. Let's get started."),
    Message.user("What is 2 + 2, 3*3, 9/5, 2*20, 14/2?"),
    Message(
        role="assistant",
        content=[ToolUse(id="1", name="add", parameters={"a": 2, "b": 2})],
    ),
    Message(role="user", content=[ToolResult(tool_use_id="1", output="4")]),
    Message(
        role="assistant",
        content=[ToolUse(id="2", name="multiply", parameters={"a": 3, "b": 3})],
    ),
    Message(role="user", content=[ToolResult(tool_use_id="2", output="9")]),
    Message(
        role="assistant",
        content=[ToolUse(id="3", name="divide", parameters={"a": 9, "b": 5})],
    ),
    Message(role="user", content=[ToolResult(tool_use_id="3", output="1.8")]),
    Message(
        role="assistant",
        content=[ToolUse(id="4", name="multiply", parameters={"a": 2, "b": 20})],
    ),
    Message(role="user", content=[ToolResult(tool_use_id="4", output="40")]),
    Message(
        role="assistant",
        content=[ToolUse(id="5", name="divide", parameters={"a": 14, "b": 2})],
    ),
    Message(role="user", content=[ToolResult(tool_use_id="5", output="7")]),
    Message.assistant("I'm done calculating the answers to your math questions."),
    Message.user("Can you also help with my science homework?"),
    Message.assistant("Yes, I can help with science too."),
    Message.user("What is the speed of light? The frequency of a photon? The mass of an electron?"),
    Message(
        role="assistant",
        content=[ToolUse(id="6", name="speed_of_light", parameters={})],
    ),
    Message(role="user", content=[ToolResult(tool_use_id="6", output="299,792,458 m/s")]),
    Message(
        role="assistant",
        content=[ToolUse(id="7", name="photon_frequency", parameters={})],
    ),
    Message(role="user", content=[ToolResult(tool_use_id="7", output="2.418 x 10^14 Hz")]),
    Message(role="assistant", content=[ToolUse(id="8", name="electron_mass", parameters={})]),
    Message(
        role="user",
        content=[ToolResult(tool_use_id="8", output="9.10938356 x 10^-31 kg")],
    ),
    Message.assistant("I'm done calculating the answers to your science questions."),
    Message.user("That's great! How about history?"),
    Message.assistant("Of course, I can help with history as well."),
    Message.user("Thanks! You're very helpful."),
    Message.assistant("You're welcome! I'm here to help."),
]


class AnotherMockProvider:
    def __init__(self):
        self.sequence = MESSAGE_SEQUENCE
        self.current_index = 1
        self.summarize_next = False
        self.summarized_count = 0

    def complete(self, model, system, messages, tools):
        system_prompt_tokens = 100
        input_token_count = system_prompt_tokens

        message = self.sequence[self.current_index]
        if self.summarize_next:
            text = "Summary message here"
            self.summarize_next = False
            self.summarized_count += 1
            return Message.assistant(text=text), Usage(
                # in this case, input tokens can really be whatever
                input_tokens=40,
                output_tokens=len(text) * 2,
                total_tokens=40 + len(text) * 2,
            )

        if len(messages) > 0 and type(messages[0].content[0]) is ToolResult:
            raise ValueError("ToolResult should not be the first message")

        if len(messages) == 1 and messages[0].text == "a":
            # adding a +1 for the "a"
            return Message.assistant("Getting system prompt size"), Usage(
                input_tokens=80 + 1, output_tokens=20, total_tokens=system_prompt_tokens + 1
            )

        for i in range(len(messages)):
            if type(messages[i].content[0]) in (ToolResult, ToolUse):
                input_token_count += 10
            else:
                input_token_count += len(messages[i].text) * 2

        if type(message.content[0]) in (ToolResult, ToolUse):
            output_tokens = 10
        else:
            output_tokens = len(message.text) * 2

        total_tokens = input_token_count + output_tokens
        if total_tokens > 300:
            self.summarize_next = True
        usage = Usage(
            input_tokens=input_token_count,
            output_tokens=output_tokens,
            total_tokens=total_tokens,
        )
        self.current_index += 2
        return message, usage


@pytest.fixture
def conversation_exchange_instance():
    ex = Exchange(
        provider=AnotherMockProvider(),
        model="test-model",
        system="test-system",
        moderator=ContextSummarizer(max_tokens=300),
        # TODO: make it work with an offset so we don't have to send off requests basically
        # at every generate step
    )
    return ex


def test_summarizer_generic_conversation(conversation_exchange_instance: Exchange):
    i = 0
    while i < len(MESSAGE_SEQUENCE):
        next_message = MESSAGE_SEQUENCE[i]
        conversation_exchange_instance.add(next_message)
        message = conversation_exchange_instance.generate()
        if message.text != "Summary message here":
            i += 2
    checkpoints = conversation_exchange_instance.checkpoint_data.checkpoints
    assert conversation_exchange_instance.checkpoint_data.total_token_count == 570
    assert len(checkpoints) == 10
    assert len(conversation_exchange_instance.messages) == 10
    assert checkpoints[0].start_index == 20
    assert checkpoints[0].end_index == 20
    assert checkpoints[-1].start_index == 29
    assert checkpoints[-1].end_index == 29
    assert conversation_exchange_instance.checkpoint_data.message_index_offset == 20
    assert conversation_exchange_instance.provider.summarized_count == 12
    assert conversation_exchange_instance.moderator.system_prompt_token_count == 100
