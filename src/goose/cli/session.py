import traceback
from pathlib import Path
from typing import Any, Dict, List, Optional

from exchange import Message, ToolResult, ToolUse, Text
from prompt_toolkit.shortcuts import confirm
from rich import print
from rich.console import RenderableType
from rich.live import Live
from rich.markdown import Markdown
from rich.panel import Panel
from rich.status import Status

from goose.build import build_exchange
from goose.cli.config import (
    default_profiles,
    ensure_config,
    read_config,
    session_path,
)
from goose.cli.prompt.goose_prompt_session import GoosePromptSession
from goose.notifier import Notifier
from goose.profile import Profile
from goose.utils import droid, load_plugins
from goose.utils.session_file import read_from_file, write_to_file

RESUME_MESSAGE = "I see we were interrupted. How can I help you?"


def load_provider() -> str:
    # We try to infer a provider, by going in order of what will auth
    providers = load_plugins(group="exchange.provider")
    for provider, cls in providers.items():
        try:
            cls.from_env()
            print(Panel(f"[green]Detected an available provider: [/]{provider}"))
            return provider
        except Exception:
            pass
    else:
        # TODO link to auth docs
        print(
            Panel(
                "[red]Could not authenticate any providers[/]\n"
                + "Returning a default pointing to openai, but you will need to set an API token env variable."
            )
        )
        return "openai"


def load_profile(name: Optional[str]) -> Profile:
    if name is None:
        name = "default"

    # If the name is one of the default values, we ensure a valid configuration
    if name in default_profiles():
        return ensure_config(name)

    # Otherwise this is a custom config and we return it from the config file
    return read_config()[name]


class SessionNotifier(Notifier):
    def __init__(self, status_indicator: Status) -> None:
        self.status_indicator = status_indicator
        self.live = Live(self.status_indicator, refresh_per_second=8, transient=True)

    def log(self, content: RenderableType) -> None:
        print(content)

    def status(self, status: str) -> None:
        self.status_indicator.update(status)

    def start(self) -> None:
        self.live.start()

    def stop(self) -> None:
        self.live.stop()


class Session:
    """A session handler for managing interactions between a user and the Goose exchange

    This class encapsulates the entire user interaction cycle, from input prompt to response handling,
    including interruptions and error management.
    """

    def __init__(
        self,
        name: Optional[str] = None,
        profile: Optional[str] = None,
        plan: Optional[dict] = None,
        **kwargs: Dict[str, Any],
    ) -> None:
        self.name = name
        self.status_indicator = Status("", spinner="dots")
        self.notifier = SessionNotifier(self.status_indicator)

        self.exchange = build_exchange(profile=load_profile(profile), notifier=self.notifier)

        if name is not None and self.session_file_path.exists():
            messages = self.load_session()

            if messages and messages[-1].role == "user":
                if type(messages[-1].content[-1]) is Text:
                    # remove the last user message
                    messages.pop()
                elif type(messages[-1].content[-1]) is ToolResult:
                    # if we remove this message, we would need to remove
                    # the previous assistant message as well. instead of doing
                    # that, we just add a new assistant message to prompt the user
                    messages.append(Message.assistant(RESUME_MESSAGE))
            if messages and type(messages[-1].content[-1]) is ToolUse:
                # remove the last request for a tool to be used
                messages.pop()

                # add a new assistant text message to prompt the user
                messages.append(Message.assistant(RESUME_MESSAGE))
            self.exchange.messages.extend(messages)

        if len(self.exchange.messages) == 0 and plan:
            self.setup_plan(plan=plan)

        self.prompt_session = GoosePromptSession.create_prompt_session()

    def setup_plan(self, plan: dict) -> None:
        if len(self.exchange.messages):
            raise ValueError("The plan can only be set on an empty session.")
        self.exchange.messages.append(Message.user(plan["kickoff_message"]))
        tasks = []
        if "tasks" in plan:
            tasks = [dict(description=task, status="planned") for task in plan["tasks"]]

        plan_tool_use = ToolUse(id="initialplan", name="update_plan", parameters=dict(tasks=tasks))
        self.exchange.add_tool_use(plan_tool_use)

    def process_first_message(self) -> Optional[Message]:
        # Get a first input unless it has been specified, such as by a plan
        if len(self.exchange.messages) == 0 or self.exchange.messages[-1].role == "assistant":
            user_input = self.prompt_session.get_user_input()
            if user_input.to_exit():
                return None
            return Message.user(text=user_input.text)
        return self.exchange.messages.pop()

    def run(self) -> None:
        """
        Runs the main loop to handle user inputs and responses.
        Continues until an empty string is returned from the prompt.
        """
        message = self.process_first_message()
        while message:  # Loop until no input (empty string).
            self.notifier.start()
            try:
                self.exchange.add(message)
                self.reply()  # Process the user message.
            except KeyboardInterrupt:
                self.interrupt_reply()
            except Exception:
                # rewind to right before the last user message
                self.exchange.rewind()
                print(traceback.format_exc())
                print(
                    "\n[red]The error above was an exception we were not able to handle.\n\n[/]"
                    + "These errors are often related to connection or authentication\n"
                    + "We've removed the conversation up to the most recent user message"
                    + " - [yellow]depending on the error you may be able to continue[/]"
                )
            self.notifier.stop()

            print()  # Print a newline for separation.
            user_input = self.prompt_session.get_user_input()
            message = Message.user(text=user_input.text) if user_input.to_continue() else None

        self.save_session()

    def reply(self) -> None:
        """Reply to the last user message, calling tools as needed

        Args:
            text (str): The text input from the user.
        """
        self.status_indicator.update("responding")
        response = self.exchange.generate()

        if response.text:
            print(Markdown(response.text))

        while response.tool_use:
            content = []
            for tool_use in response.tool_use:
                tool_result = self.exchange.call_function(tool_use)
                content.append(tool_result)
            self.exchange.add(Message(role="user", content=content))
            self.status_indicator.update("responding")
            response = self.exchange.generate()

            if response.text:
                print(Markdown(response.text))

    def interrupt_reply(self) -> None:
        """Recover from an interruption at an arbitrary state"""
        # Default recovery message if no user message is pending.
        recovery = "We interrupted before the next processing started."
        if self.exchange.messages and self.exchange.messages[-1].role == "user":
            # If the last message is from the user, remove it.
            self.exchange.messages.pop()
            recovery = "We interrupted before the model replied and removed the last message."

        if (
            self.exchange.messages
            and self.exchange.messages[-1].role == "assistant"
            and self.exchange.messages[-1].tool_use
        ):
            content = []
            # Append tool results as errors if interrupted.
            for tool_use in self.exchange.messages[-1].tool_use:
                content.append(
                    ToolResult(
                        tool_use_id=tool_use.id,
                        output="Interrupted by the user to make a correction",
                        is_error=True,
                    )
                )
            self.exchange.add(Message(role="user", content=content))
            recovery = f"We interrupted the existing call to {tool_use.name}. How would you like to proceed?"
            self.exchange.add(Message.assistant(recovery))
        # Print the recovery message with markup for visibility.
        print(f"[yellow]{recovery}[/]")

    @property
    def session_file_path(self) -> Path:
        return session_path(self.name)

    def save_session(self) -> None:
        """Save the current session to a file in JSON format."""
        if self.name is None:
            self.generate_session_name()

        try:
            if self.session_file_path.exists():
                if not confirm(f"Session {self.name} exists in {self.session_file_path}, overwrite?"):
                    self.generate_session_name()
            write_to_file(self.session_file_path, self.exchange.messages)
        except PermissionError as e:
            raise RuntimeError(f"Failed to save session due to permissions: {e}")
        except (IOError, OSError) as e:
            raise RuntimeError(f"Failed to save session due to I/O error: {e}")

    def load_session(self) -> List[Message]:
        """Load a session from a JSON file."""
        return read_from_file(self.session_file_path)

    def generate_session_name(self) -> None:
        user_entered_session_name = self.prompt_session.get_save_session_name()
        self.name = user_entered_session_name if user_entered_session_name else droid()
        print(f"Saving to [bold cyan]{self.session_file_path}[/bold cyan]")


if __name__ == "__main__":
    session = Session()
