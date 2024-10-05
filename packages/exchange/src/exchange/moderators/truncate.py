from __future__ import annotations

from typing import TYPE_CHECKING, List, Optional

from exchange.checkpoint import CheckpointData
from exchange.message import Message
from exchange.moderators import PassiveModerator
from exchange.moderators.base import Moderator

if TYPE_CHECKING:
    from exchange.exchange import Exchange

# currently this is the point at which we start to truncate, so
# so once we get to this token size the token count will exceed this
# by a little bit.
# TODO: make this configurable for each provider
MAX_TOKENS = 100000


class ContextTruncate(Moderator):
    def __init__(
        self,
        model: Optional[str] = None,
        max_tokens: int = MAX_TOKENS,
    ) -> None:
        self.model = model
        self.system_prompt_token_count = 0
        self.max_tokens = max_tokens
        self.last_system_prompt = None

    def rewrite(self, exchange: Exchange) -> None:
        """Truncate the exchange messages with a FIFO strategy."""
        self._update_system_prompt_token_count(exchange)

        if exchange.checkpoint_data.total_token_count < self.max_tokens:
            return

        messages_to_remove = self._get_messages_to_remove(exchange)
        for _ in range(len(messages_to_remove)):
            exchange.pop_first_message()

    def _update_system_prompt_token_count(self, exchange: Exchange) -> None:
        is_different_system_prompt = False
        if self.last_system_prompt != exchange.system:
            is_different_system_prompt = True
            self.last_system_prompt = exchange.system

        if not self.system_prompt_token_count or is_different_system_prompt:
            # calculate the system prompt tokens (includes functions etc...)
            # we use a placeholder message with one token, which we subtract later
            # this ensures compatibility with providers that require a user message
            _system_token_exchange = exchange.replace(
                messages=[Message.user("a")],
                checkpoint_data=CheckpointData(),
                moderator=PassiveModerator(),
                model=self.model if self.model else exchange.model,
            )
            _system_token_exchange.generate()
            last_system_prompt_token_count = self.system_prompt_token_count
            self.system_prompt_token_count = _system_token_exchange.checkpoint_data.total_token_count - 1

            exchange.checkpoint_data.total_token_count -= last_system_prompt_token_count
            exchange.checkpoint_data.total_token_count += self.system_prompt_token_count

    def _get_messages_to_remove(self, exchange: Exchange) -> List[Message]:
        # this keeps all the messages/checkpoints
        throwaway_exchange = exchange.replace(
            moderator=PassiveModerator(),
        )

        # get the messages that we want to remove
        messages_to_remove = []
        while throwaway_exchange.checkpoint_data.total_token_count > self.max_tokens:
            _, messages = throwaway_exchange.pop_first_checkpoint()
            messages_to_remove.extend(messages)

        while len(throwaway_exchange.messages) > 0 and throwaway_exchange.messages[0].tool_result:
            # we would need a corresponding tool use once we resume, so we pop this one off too
            # and summarize it as well
            _, messages = throwaway_exchange.pop_first_checkpoint()
            messages_to_remove.extend(messages)
        return messages_to_remove
