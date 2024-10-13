import traceback
from pathlib import Path
from typing import Any, Optional

from exchange import Message, Text, ToolResult, ToolUse
from rich import print
from rich.markdown import Markdown
from rich.panel import Panel
from rich.prompt import Prompt
from rich.status import Status

from goose._logger import get_logger, setup_logging
from goose.cli.config import LOG_PATH, ensure_config, session_path
from goose.cli.prompt.goose_prompt_session import GoosePromptSession
from goose.cli.prompt.overwrite_session_prompt import OverwriteSessionPrompt
from goose.cli.session_notifier import SessionNotifier
from goose.profile import Profile
from goose.utils import droid, load_plugins
from goose.utils._cost_calculator import get_total_cost_message
from goose.utils._create_exchange import create_exchange
from goose.utils.session_file import is_empty_session, is_existing_session, read_or_create_file, save_latest_session

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
        **kwargs: dict[str, Any],
    ) -> None:
        if name is None:
            self.name = droid()
        else:
            self.name = name
        self.profile_name = profile
        self.prompt_session = GoosePromptSession()
        self.status_indicator = Status("", spinner="dots")
        self.notifier = SessionNotifier(self.status_indicator)

        self.exchange = create_exchange(profile=load_profile(profile), notifier=self.notifier)
        setup_logging(log_file_directory=LOG_PATH, log_level=log_level)

        self.exchange.messages.extend(self._get_initial_messages())

        if len(self.exchange.messages) == 0 and plan:
            self.setup_plan(plan=plan)

        self.prompt_session = GoosePromptSession()

    def _get_initial_messages(self) -> list[Message]:
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

    def single_pass(self, initial_message: str) -> None:
        """
        Handles a single input message and processes a reply
        without entering a loop for additional inputs.

        Args:
            initial_message (str): The initial user message to process.
        """
        profile = self.profile_name or "default"
        print(f"[dim]starting session | name:[cyan]{self.name}[/]  profile:[cyan]{profile}[/]")
        print(f"[dim]saving to {self.session_file_path}")
        print()

        # Process initial message
        message = Message.user(initial_message)

        self.exchange.add(message)
        self.reply()  # Process the user message

        save_latest_session(self.session_file_path, self.exchange.messages)
        print()  # Print a newline for separation.

        print(f"[dim]ended run | name:[cyan]{self.name}[/]  profile:[cyan]{profile}[/]")
        print(f"[dim]to resume: [magenta]goose session resume {self.name} --profile {profile}[/][/]")

    def run(self, new_session: bool = True) -> None:
        """
        Runs the main loop to handle user inputs and responses.
        Continues until an empty string is returned from the prompt.

        Args:
            new_session (bool): True when starting a new session, False when resuming.
        """
        if is_existing_session(self.session_file_path) and new_session:
            self._prompt_overwrite_session()

        profile_name = self.profile_name or "default"
        print(f"[dim]starting session | name: [cyan]{self.name}[/cyan]  profile: [cyan]{profile_name}[/cyan][/dim]")
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

        self._remove_empty_session()
        self._log_cost()

    def reply(self) -> None:
        """Reply to the last user message, calling tools as needed"""
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

    def load_session(self) -> list[Message]:
        return read_or_create_file(self.session_file_path)

    def _log_cost(self) -> None:
        get_logger().info(get_total_cost_message(self.exchange.get_token_usage()))
        print(f"[dim]you can view the cost and token usage in the log directory {LOG_PATH}[/]")

    def _prompt_overwrite_session(self) -> None:
        print(f"[yellow]Session already exists at {self.session_file_path}.[/]")

        choice = OverwriteSessionPrompt.ask("Enter your choice", show_choices=False)
        match choice:
            case "y" | "yes":
                print("Overwriting existing session")

            case "n" | "no":
                while True:
                    new_session_name = Prompt.ask("Enter a new session name")
                    if not is_existing_session(session_path(new_session_name)):
                        self.name = new_session_name
                        break
                    print(f"[yellow]Session '{new_session_name}' already exists[/]")

            case "r" | "resume":
                self.exchange.messages.extend(self.load_session())

    def _remove_empty_session(self) -> bool:
        """
        Removes the session file only when it's empty.

        Note: This is because a session file is created at the start of the run
        loop. When a user aborts before their first message empty session files
        will be created, causing confusion when resuming sessions (which
        depends on most recent mtime and is non-empty).

        Returns:
            bool: True if the session file was removed, False otherwise.
        """
        logger = get_logger()
        try:
            if is_empty_session(self.session_file_path):
                logger.debug(f"deleting empty session file: {self.session_file_path}")
                self.session_file_path.unlink()
                return True
        except Exception as e:
            logger.error(f"error deleting empty session file: {e}")
        return False


if __name__ == "__main__":
    session = Session()
