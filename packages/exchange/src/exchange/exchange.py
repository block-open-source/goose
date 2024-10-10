import json
import traceback
from copy import deepcopy
from typing import Any, Dict, List, Mapping, Tuple

from attrs import define, evolve, field, Factory
from tiktoken import get_encoding

from exchange.checkpoint import Checkpoint, CheckpointData
from exchange.content import Text, ToolResult, ToolUse
from exchange.message import Message
from exchange.moderators import Moderator
from exchange.moderators.truncate import ContextTruncate
from exchange.providers import Provider, Usage
from exchange.tool import Tool
from exchange.token_usage_collector import _token_usage_collector


def validate_tool_output(output: str) -> None:
    """Validate tool output for the given model"""
    max_output_chars = 2**20
    max_output_tokens = 16000
    encoder = get_encoding("cl100k_base")
    if len(output) > max_output_chars or len(encoder.encode(output)) > max_output_tokens:
        raise ValueError("This tool call created an output that was too long to handle!")


@define(frozen=True)
class Exchange:
    """An exchange of messages with an LLM

    The exchange class is meant to be largely immutable, with only the message list
    growing once constructed. Use .replace to alter the model, tools, etc.

    The exchange supports tool usage, calling tools and letting the model respond when
    using the .reply method. It handles most forms of errors and sends those errors back
    to the model, to let it attempt to recover.
    """

    provider: Provider
    model: str
    system: str
    moderator: Moderator = field(default=ContextTruncate())
    tools: Tuple[Tool] = field(factory=tuple, converter=tuple)
    messages: List[Message] = field(factory=list)
    checkpoint_data: CheckpointData = field(factory=CheckpointData)
    generation_args: dict = field(default=Factory(dict))

    @property
    def _toolmap(self) -> Mapping[str, Tool]:
        return {tool.name: tool for tool in self.tools}

    def replace(self, **kwargs: Dict[str, Any]) -> "Exchange":
        """Make a copy of the exchange, replacing any passed arguments"""
        # TODO: ensure that the checkpoint data is updated correctly. aka,
        # if we replace the messages, we need to update the checkpoint data
        # if we change the model, we need to update the checkpoint data (?)

        if kwargs.get("messages") is None:
            kwargs["messages"] = deepcopy(self.messages)
        if kwargs.get("checkpoint_data") is None:
            kwargs["checkpoint_data"] = deepcopy(
                self.checkpoint_data,
            )
        return evolve(self, **kwargs)

    def add(self, message: Message) -> None:
        """Add a message to the history."""
        if self.messages and message.role == self.messages[-1].role:
            raise ValueError("Messages in the exchange must alternate between user and assistant")
        self.messages.append(message)

    def generate(self) -> Message:
        """Generate the next message."""
        self.moderator.rewrite(self)
        message, usage = self.provider.complete(
            self.model,
            self.system,
            messages=self.messages,
            tools=self.tools,
            **self.generation_args,
        )
        self.add(message)
        self.add_checkpoints_from_usage(usage)  # this has to come after adding the response

        # TODO: also call `rewrite` here, as this will make our
        # messages *consistently* below the token limit. this currently
        # is not the case because we could append a large message after calling
        # `rewrite` above.
        # self.moderator.rewrite(self)

        _token_usage_collector.collect(self.model, usage)
        return message

    def reply(self, max_tool_use: int = 128) -> Message:
        """Get the reply from the underlying model.

        This will process any requests for tool calls, calling them immediately, and
        storing the intermediate tool messages in the queue. It will return after the
        first response that does not request a tool use

        Args:
            max_tool_use: The maximum number of tool calls to make before returning. Defaults to 128.
        """
        if max_tool_use <= 0:
            raise ValueError("max_tool_use must be greater than 0")
        response = self.generate()
        curr_iter = 1  # generate() already called once
        while response.tool_use:
            content = []
            for tool_use in response.tool_use:
                tool_result = self.call_function(tool_use)
                content.append(tool_result)
            self.add(Message(role="user", content=content))

            # We've reached the limit of tool calls - break out of the loop
            if curr_iter >= max_tool_use:
                # At this point, the most recent message is `Message(role='user', content=ToolResult(...))`
                response = Message.assistant(
                    f"We've stopped executing additional tool cause because we reached the limit of {max_tool_use}",
                )
                self.add(response)
                break
            else:
                response = self.generate()
                curr_iter += 1

        return response

    def call_function(self, tool_use: ToolUse) -> ToolResult:
        """Call the function indicated by the tool use"""
        tool = self._toolmap.get(tool_use.name)

        if tool is None or tool_use.is_error:
            output = f"ERROR: Failed to use tool {tool_use.id}.\nDo NOT use the same tool name and parameters again - that will lead to the same error."  # noqa: E501

            if tool_use.is_error:
                output += f"\n{tool_use.error_message}"
            elif tool is None:
                valid_tool_names = ", ".join(self._toolmap.keys())
                output += f"\nNo tool exists with the name '{tool_use.name}'. Valid tool names are: {valid_tool_names}"

            return ToolResult(tool_use_id=tool_use.id, output=output, is_error=True)

        try:
            if isinstance(tool_use.parameters, dict):
                output = json.dumps(tool.function(**tool_use.parameters))
            elif isinstance(tool_use.parameters, list):
                output = json.dumps(tool.function(*tool_use.parameters))
            else:
                raise ValueError(
                    f"The provided tool parameters, {tool_use.parameters} could not be interpreted as a mapping of arguments."  # noqa: E501
                )

            validate_tool_output(output)

            is_error = False
        except Exception as e:
            tb = traceback.format_exc()
            output = str(tb) + "\n" + str(e)
            is_error = True

        return ToolResult(tool_use_id=tool_use.id, output=output, is_error=is_error)

    def add_tool_use(self, tool_use: ToolUse) -> None:
        """Manually add a tool use and corresponding result

        This will call the implied function and add an assistant
        message requesting the ToolUse and a user message with the ToolResult
        """
        tool_result = self.call_function(tool_use)
        self.add(Message(role="assistant", content=[tool_use]))
        self.add(Message(role="user", content=[tool_result]))

    def add_checkpoints_from_usage(self, usage: Usage) -> None:
        """
        Add checkpoints to the exchange based on the token counts of the last two
        groups of messages, as well as the current token total count of the exchange
        """
        # we know we just appended one message as the response from the LLM
        # so we need to create two checkpoints as we know the token counts
        # of the last two groups of messages:
        # 1. from the last checkpoint to the most recent user message
        # 2. the most recent assistant message
        last_checkpoint_end_index = (
            self.checkpoint_data.checkpoints[-1].end_index - self.checkpoint_data.message_index_offset
            if len(self.checkpoint_data.checkpoints) > 0
            else -1
        )
        new_start_index = last_checkpoint_end_index + 1

        # here, our self.checkpoint_data.total_token_count is the previous total token count from the last time
        # that we performed a request. if we subtract this value from the input_tokens from our
        # latest response, we know how many tokens our **1** from above is.
        first_block_token_count = usage.input_tokens - self.checkpoint_data.total_token_count
        second_block_token_count = usage.output_tokens

        if len(self.messages) - new_start_index > 1:
            # this will occur most of the time, as we will have one new user message and one
            # new assistant message.

            self.checkpoint_data.checkpoints.append(
                Checkpoint(
                    start_index=new_start_index + self.checkpoint_data.message_index_offset,
                    # end index below is equivalent to the second last message. why? becuase
                    # the last message is the assistant message that we add below. we need to also
                    # track the token count of the user message sent.
                    end_index=len(self.messages) - 2 + self.checkpoint_data.message_index_offset,
                    token_count=first_block_token_count,
                )
            )
        self.checkpoint_data.checkpoints.append(
            Checkpoint(
                start_index=len(self.messages) - 1 + self.checkpoint_data.message_index_offset,
                end_index=len(self.messages) - 1 + self.checkpoint_data.message_index_offset,
                token_count=second_block_token_count,
            )
        )

        # TODO: check if the front of the checkpoints doesn't overlap with
        # the first message. if so, we are missing checkpoint data from
        # message[0] to message[checkpoint_data.checkpoints[0].start_index]
        # we can fill in this data by performing an extra request and doing some math
        self.checkpoint_data.total_token_count = usage.total_tokens

    def pop_last_message(self) -> Message:
        """Pop the last message from the exchange, handling checkpoints correctly"""
        if (
            len(self.checkpoint_data.checkpoints) > 0
            and self.checkpoint_data.last_message_index > len(self.messages) - 1
        ):
            raise ValueError("Our checkpoint data is out of sync with our message data")
        if (
            len(self.checkpoint_data.checkpoints) > 0
            and self.checkpoint_data.last_message_index == len(self.messages) - 1
        ):
            # remove the last checkpoint, because we no longer know the token count of it's contents.
            # note that this is not the same as reverting to the last checkpoint, as we want to
            # keep the messages from the last checkpoint. they will have a new checkpoint created for
            # them when we call generate() again
            self.checkpoint_data.pop()
        self.messages.pop()

    def pop_first_message(self) -> Message:
        """Pop the first message from the exchange, handling checkpoints correctly"""
        if len(self.messages) == 0:
            raise ValueError("There are no messages to pop")
        if len(self.checkpoint_data.checkpoints) == 0:
            raise ValueError("There must be at least one checkpoint to pop the first message")

        # get the start and end indexes of the first checkpoint, use these to remove message
        first_checkpoint = self.checkpoint_data.checkpoints[0]
        first_checkpoint_start_index = first_checkpoint.start_index - self.checkpoint_data.message_index_offset

        # check if the first message is part of the first checkpoint
        if first_checkpoint_start_index == 0:
            # remove this checkpoint, as it no longer has any messages
            self.checkpoint_data.pop(0)

        self.messages.pop(0)
        self.checkpoint_data.message_index_offset += 1

        if len(self.checkpoint_data.checkpoints) == 0:
            # we've removed all the checkpoints, so we need to reset the message index offset
            self.checkpoint_data.message_index_offset = 0

    def pop_last_checkpoint(self) -> Tuple[Checkpoint, List[Message]]:
        """
        Reverts the exchange back to the last checkpoint, removing associated messages
        """
        removed_checkpoint = self.checkpoint_data.checkpoints.pop()
        # pop messages until we reach the start of the next checkpoint
        messages = []
        while len(self.messages) > removed_checkpoint.start_index - self.checkpoint_data.message_index_offset:
            messages.append(self.messages.pop())
        return removed_checkpoint, messages

    def pop_first_checkpoint(self) -> Tuple[Checkpoint, List[Message]]:
        """
        Pop the first checkpoint from the exchange, removing associated messages
        """
        if len(self.checkpoint_data.checkpoints) == 0:
            raise ValueError("There are no checkpoints to pop")
        first_checkpoint = self.checkpoint_data.pop(0)

        # remove messages until we reach the start of the next checkpoint
        messages = []
        stop_at_index = first_checkpoint.end_index - self.checkpoint_data.message_index_offset
        for _ in range(stop_at_index + 1):  # +1 because it's inclusive
            messages.append(self.messages.pop(0))
            self.checkpoint_data.message_index_offset += 1

        if len(self.checkpoint_data.checkpoints) == 0:
            # we've removed all the checkpoints, so we need to reset the message index offset
            self.checkpoint_data.message_index_offset = 0
        return first_checkpoint, messages

    def prepend_checkpointed_message(self, message: Message, token_count: int) -> None:
        """Prepend a message to the exchange, updating the checkpoint data"""
        self.messages.insert(0, message)
        new_index = max(0, self.checkpoint_data.message_index_offset - 1)
        self.checkpoint_data.checkpoints.insert(
            0,
            Checkpoint(
                start_index=new_index,
                end_index=new_index,
                token_count=token_count,
            ),
        )
        self.checkpoint_data.message_index_offset = new_index

    def rewind(self) -> None:
        if not self.messages:
            return

        # we remove messages until we find the last user text message
        while not (self.messages[-1].role == "user" and type(self.messages[-1].content[-1]) is Text):
            self.pop_last_message()

        # now we remove that last user text message, putting us at a good point
        # to ask the user for their input again
        if self.messages:
            self.pop_last_message()

    @property
    def is_allowed_to_call_llm(self) -> bool:
        """
        Returns True if the exchange is allowed to call the LLM, False otherwise
        """
        # TODO: reconsider whether this function belongs here and whether it is necessary
        # Some models will have different requirements than others, so it may be better for
        # this to be a required method of the provider instead.
        return len(self.messages) > 0 and self.messages[-1].role == "user"

    def get_token_usage(self) -> Dict[str, Usage]:
        return _token_usage_collector.get_token_usage_group_by_model()
