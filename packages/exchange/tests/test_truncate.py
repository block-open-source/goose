import pytest
from exchange import Exchange
from exchange.content import ToolResult, ToolUse
from exchange.message import Message
from exchange.moderators.truncate import ContextTruncate
from exchange.providers import Provider, Usage

MAX_TOKENS = 300
SYSTEM_PROMPT_TOKENS = 100

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


class TruncateLinearProvider(Provider):
    def __init__(self):
        self.sequence = MESSAGE_SEQUENCE
        self.current_index = 1
        self.summarize_next = False
        self.summarized_count = 0

    def complete(self, model, system, messages, tools):
        input_token_count = SYSTEM_PROMPT_TOKENS

        message = self.sequence[self.current_index]

        if len(messages) > 0 and type(messages[0].content[0]) is ToolResult:
            raise ValueError("ToolResult should not be the first message")

        if len(messages) == 1 and messages[0].text == "a":
            # adding a +1 for the "a"
            return Message.assistant("Getting system prompt size"), Usage(
                input_tokens=80 + 1, output_tokens=20, total_tokens=SYSTEM_PROMPT_TOKENS + 1
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
        provider=TruncateLinearProvider(),
        model="test-model",
        system="test-system",
        moderator=ContextTruncate(max_tokens=500),
    )
    return ex


def test_truncate_on_generic_conversation(conversation_exchange_instance: Exchange):
    i = 0
    while i < len(MESSAGE_SEQUENCE):
        next_message = MESSAGE_SEQUENCE[i]
        conversation_exchange_instance.add(next_message)
        message = conversation_exchange_instance.generate()
        if message.text != "Summary message here":
            i += 2
        # ensure the total token count is not anything exhorbitant
        assert conversation_exchange_instance.checkpoint_data.total_token_count < 700
        assert conversation_exchange_instance.moderator.system_prompt_token_count == 100
