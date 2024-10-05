import os
from goose.toolkit.utils import render_template
from pathlib import Path
from exchange.content import Text
from exchange.exchange import Exchange
from exchange.message import Message
from exchange.moderators import Moderator
from exchange.moderators.passive import PassiveModerator
from exchange.moderators.truncate import ContextTruncate
from goose.synopsis.system import system


class Synopsis(Moderator):
    """Synopsis rewrites the chat into a single input message after every reply

    The goal of synopsis is to remove history artifacts that eat up the context budget
    and provide opportunities for the model to attend to the wrong content.

    When?
       - After every message (mostly tool use), we apply automatic context management
       - After every user input, we use an LLM to curate the context
    This is a compromise for speed, where we do fast updates as frequently as possible

    What?
      - [Automatic] Current sytem state
      - [Automatic] Current file context
      - (TODO) [Automatic] Loaded memories
      - [Curated] Summary of the discussion so far
      - [Curated] Summary of the plan, next step to solve

    At the moment, this is tightly coupled to the SynopsisDeveloper toolkit as base. We
    could revisit how that works, because this application shows some limitations in how the
    goose state is managed.
    """

    def __init__(self) -> None:
        super().__init__()
        self.current_summary = ""
        self.current_plan = ""
        self.originals = []

        hints = []
        hints_path = Path(".goosehints")
        home_hints_path = Path.home() / ".config/goose/.goosehints"
        if hints_path.is_file():
            hints.append(render_template(hints_path))
        if home_hints_path.is_file():
            hints.append(render_template(home_hints_path))
        self.hints = "\n".join(hints)

    def rewrite(self, exchange: Exchange) -> None:
        # Get the last message, which would be either a user text or a user tool use
        last_message: Message = exchange.messages[-1]

        # The first message is the synopsis, there could be multiple tool usages until we encounter a new user message
        # [synopsis, tool_use, tool_result, tool_use, tool_result, ..., assistant_message, user_message]

        if isinstance(last_message.content[0], Text):
            # We're in the state:
            # [synopsis, tool_use, tool_result, ..., user_message]
            # and we'll reset it to:
            # [synopsis]

            # keep track of the original messages before we reset
            if not self.originals:
                self.originals.extend(exchange.messages)
                if len(exchange.messages) > 1:
                    # we are resuming an existing session, and need to restore state
                    system.restore(exchange.messages)
            else:
                self.originals.extend(exchange.messages[1:])

            exchange.messages.clear()
            exchange.add(self.get_synopsis(exchange, summarize=True, plan=True))
        else:
            # We're in the state
            # [synopsis, ..., tool_use, tool_result]
            # and we'll keep going but updated the synopsis
            # [new_synopsis, ..., tool_use, tool_result]
            exchange.messages[0] = self.get_synopsis(exchange)

    def get_synopsis(self, exchange: Exchange, summarize: bool = False, plan: bool = False) -> Message:
        if summarize:
            self.current_summary = self.summarize(exchange)

        if plan:
            self.current_plan = self.plan(exchange)

        return Message.load("synopsis.md", synopsis=self, system=system)

    def summarize(self, exchange: Exchange) -> str:
        message = Message.load("summarize.md", synopsis=self, messages=self.originals, exchange=exchange, system=system)
        model = os.environ.get("SUMMARIZER", exchange.model)
        new_exchange = exchange.replace(moderator=ContextTruncate(), tools=(), system="", messages=[], model=model)
        new_exchange.add(message)
        return new_exchange.generate().content[0].text

    def plan(self, exchange: Exchange) -> str:
        message = Message.load("plan.md", synopsis=self, messages=self.originals, exchange=exchange, system=system)
        model = os.environ.get("PLANNER", exchange.model)
        new_exchange = exchange.replace(moderator=PassiveModerator(), tools=(), system="", messages=[], model=model)
        new_exchange.add(message)
        return new_exchange.generate().content[0].text
