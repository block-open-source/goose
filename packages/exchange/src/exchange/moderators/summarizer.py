from typing import Type

from exchange import Message
from exchange.checkpoint import CheckpointData
from exchange.moderators import ContextTruncate, PassiveModerator


class ContextSummarizer(ContextTruncate):
    def rewrite(self, exchange: Type["exchange.exchange.Exchange"]) -> None:  # noqa: F821
        """Summarize the context history up to the last few messages in the exchange"""

        self._update_system_prompt_token_count(exchange)

        # TODO: use an offset for summarization
        if exchange.checkpoint_data.total_token_count < self.max_tokens:
            return

        messages_to_summarize = self._get_messages_to_remove(exchange)
        num_messages_to_remove = len(messages_to_summarize)

        # the llm will throw an error if the last message isn't a user message
        if messages_to_summarize[-1].role == "assistant" and (not messages_to_summarize[-1].tool_use):
            messages_to_summarize.append(Message.user("Summarize our the above conversation"))

        summarizer_exchange = exchange.replace(
            system=Message.load("summarizer.jinja").text,
            moderator=PassiveModerator(),
            model=self.model,
            messages=messages_to_summarize,
            checkpoint_data=CheckpointData(),
        )

        # get the summarized content and the tokens associated with this content
        summary = summarizer_exchange.reply()
        summary_checkpoint = summarizer_exchange.checkpoint_data.checkpoints[-1]

        # remove the checkpoints that were summarized from the original exchange
        for _ in range(num_messages_to_remove):
            exchange.pop_first_message()

        # insert summary as first message/checkpoint
        if len(exchange.messages) == 0 or exchange.messages[0].role == "assistant":
            summary_message = Message.user(summary.text)
        else:
            summary_message = Message.assistant(summary.text)
        exchange.prepend_checkpointed_message(summary_message, summary_checkpoint.token_count)
