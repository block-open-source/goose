from typing import Optional

from prompt_toolkit import PromptSession
from prompt_toolkit.formatted_text import FormattedText
from prompt_toolkit.validation import DummyValidator

from goose.cli.prompt.create import create_prompt
from goose.cli.prompt.prompt_validator import PromptValidator
from goose.cli.prompt.user_input import PromptAction, UserInput


class GoosePromptSession:
    def __init__(self, prompt_session: PromptSession) -> None:
        self.prompt_session = prompt_session

    @staticmethod
    def create_prompt_session() -> "GoosePromptSession":
        return GoosePromptSession(create_prompt())

    def get_user_input(self) -> "UserInput":
        try:
            text = FormattedText([("#00AEAE", "G❯ ")])  # Define the prompt style and text.
            message = self.prompt_session.prompt(text, validator=PromptValidator(), validate_while_typing=False)
            if message.strip() in ("exit", ":q"):
                return UserInput(PromptAction.EXIT)
            return UserInput(PromptAction.CONTINUE, message)
        except (EOFError, KeyboardInterrupt):
            return UserInput(PromptAction.EXIT)

    def get_save_session_name(self) -> Optional[str]:
        return self.prompt_session.prompt(
            "Enter a name to save this session under. A name will be generated for you if empty: ",
            validator=DummyValidator(),
        )
