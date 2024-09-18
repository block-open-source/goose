from typing import Optional

from prompt_toolkit import PromptSession
from prompt_toolkit.document import Document
from prompt_toolkit.formatted_text import FormattedText
from prompt_toolkit.validation import DummyValidator

from goose.cli.prompt.create import create_prompt
from goose.cli.prompt.lexer import PromptLexer
from goose.cli.prompt.prompt_validator import PromptValidator
from goose.cli.prompt.user_input import PromptAction, UserInput
from goose.command import get_commands


class GoosePromptSession:
    def __init__(self) -> None:
        # instantiate the commands available in the prompt
        self.commands = dict()
        command_plugins = get_commands()
        for command, command_cls in command_plugins.items():
            self.commands[command] = command_cls()

        # the main prompt session that is used to interact with the llm
        self.main_prompt_session = create_prompt(self.commands)

        # a text-only prompt session that doesn't contain any commands
        self.text_prompt_session = PromptSession()

    def get_message_after_commands(self, message: str) -> str:
        lexer = PromptLexer(command_names=list(self.commands.keys()))
        doc = Document(message)
        lines = []
        # iterate through each line of the document
        for line_num in range(len(doc.lines)):
            classes_in_line = lexer.lex_document(doc)(line_num)
            line_result = []
            i = 0
            while i < len(classes_in_line):
                # if a command is found and it is not the last part of the line
                if classes_in_line[i][0] == "class:command" and i + 1 < len(classes_in_line):
                    # extract the command name
                    command_name = classes_in_line[i][1].strip("/").strip(":")
                    # get the value following the command
                    if classes_in_line[i + 1][0] == "class:parameter":
                        command_value = classes_in_line[i + 1][1]
                    else:
                        command_value = ""

                    # execute the command with the given argument, expecting a return value
                    value_after_execution = self.commands[command_name].execute(command_value, message)

                    # if the command returns None, raise an error - this should never happen
                    # since the command should always return a string
                    if value_after_execution is None:
                        raise ValueError(f"Command {command_name} returned None")

                    # append the result of the command execution to the line results
                    line_result.append(value_after_execution)
                    i += 1

                # if the part is plain text, just append it to the line results
                elif classes_in_line[i][0] == "class:text":
                    line_result.append(classes_in_line[i][1])
                i += 1

            # join all processed parts of the current line and add it to the lines list
            lines.append("".join(line_result))

        # join all processed lines into a single string with newline characters and return
        return "\n".join(lines)

    def get_user_input(self) -> "UserInput":
        try:
            text = FormattedText([("#00AEAE", "G❯ ")])  # Define the prompt style and text.
            message = self.main_prompt_session.prompt(text, validator=PromptValidator(), validate_while_typing=False)
            if message.strip() in ("exit", ":q"):
                return UserInput(PromptAction.EXIT)

            message = self.get_message_after_commands(message)
            return UserInput(PromptAction.CONTINUE, message)
        except (EOFError, KeyboardInterrupt):
            return UserInput(PromptAction.EXIT)

    def get_save_session_name(self) -> Optional[str]:
        return self.text_prompt_session.prompt(
            "Enter a name to save this session under. A name will be generated for you if empty: ",
            validator=DummyValidator(),
        ).strip(" ")
