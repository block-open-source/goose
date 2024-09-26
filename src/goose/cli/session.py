import traceback
from pathlib import Path
from typing import Any, Dict, List, Optional

from exchange import Message, ToolResult, ToolUse, Text
from rich import print
from rich.console import RenderableType
from rich.live import Live
from rich.markdown import Markdown
from rich.panel import Panel
from rich.status import Status

from goose.build import build_exchange
from goose.cli.config import ensure_config, session_path, LOG_PATH
from goose._logger import get_logger, setup_logging
from goose.cli.prompt.goose_prompt_session import GoosePromptSession
from goose.notifier import Notifier
from goose.profile import Profile
from goose.utils import droid, load_plugins
from goose.utils._cost_calculator import get_total_cost_message
from goose.utils.session_file import read_or_create_file, save_latest_session

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
    _, profile = ensure_config(name)
    return profile


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
        log_level: Optional[str] = "INFO",
        **kwargs: Dict[str, Any],
    ) -> None:
        if name is None:
            self.name = droid()
        else:
            self.name = name
        self.profile = profile
        self.status_indicator = Status("", spinner="dots")
        self.notifier = SessionNotifier(self.status_indicator)

        self.exchange = build_exchange(profile=load_profile(profile), notifier=self.notifier)
        setup_logging(log_file_directory=LOG_PATH, log_level=log_level)

        self.exchange.messages.extend(self._get_initial_messages())

        if len(self.exchange.messages) == 0 and plan:
            self.setup_plan(plan=plan)

        self.prompt_session = GoosePromptSession()

    def _get_initial_messages(self) -> List[Message]:
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
        return messages

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
        print(f"[dim]starting session | name:[cyan]{self.name}[/]  profile:[cyan]{self.profile or 'default'}[/]")
        print(f"[dim]saving to {self.session_file_path}")
        print()
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
            save_latest_session(self.session_file_path, self.exchange.messages)
            print()  # Print a newline for separation.
            user_input = self.prompt_session.get_user_input()
            message = Message.user(text=user_input.text) if user_input.to_continue() else None

        self._log_cost()

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

    def load_session(self) -> List[Message]:
        return read_or_create_file(self.session_file_path)

    def _log_cost(self) -> None:
        get_logger().info(get_total_cost_message(self.exchange.get_token_usage()))
        print(f"[dim]you can view the cost and token usage in the log directory {LOG_PATH}")


if __name__ == "__main__":
    session = Session()
