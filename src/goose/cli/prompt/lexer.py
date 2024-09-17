import re
from typing import Callable, List, Tuple

from prompt_toolkit.document import Document
from prompt_toolkit.lexers import Lexer


# These are lexers for the commands in the prompt. This is how we
# are extracting the different parts of a command (here, used for styling),
# but likely will be used to parse the command as well in the future.


def completion_for_command(target_string: str) -> re.Pattern[str]:
    escaped_string = re.escape(target_string)
    vals = [f"(?:{escaped_string[:i]}(?=$))" for i in range(len(escaped_string), 0, -1)]
    return re.compile(rf'(?<!\S)\/({"|".join(vals)})(?:\s^|$)')


def command_itself(target_string: str) -> re.Pattern[str]:
    escaped_string = re.escape(target_string)
    return re.compile(rf"(?<!\S)(\/{escaped_string}:?)")


def value_for_command(command_string: str) -> re.Pattern[str]:
    escaped_string = re.escape(command_string + ":")
    return re.compile(rf"(?<=(?<!\S)\/{escaped_string})(?:(?:\"(.*?)(\"|$))|([^\s]*))")


class PromptLexer(Lexer):
    def __init__(self, command_names: List[str]) -> None:
        self.patterns = []
        for command_name in command_names:
            self.patterns.append((completion_for_command(command_name), "class:command"))
            self.patterns.append((value_for_command(command_name), "class:parameter"))
            self.patterns.append((command_itself(command_name), "class:command"))

    def lex_document(self, document: Document) -> Callable[[int], list]:
        def get_line_tokens(line_number: int) -> Tuple[str, str]:
            line = document.lines[line_number]
            tokens = []

            i = 0
            while i < len(line):
                match = None
                for pattern, token in self.patterns:
                    match = pattern.match(line, i)
                    if match:
                        tokens.append((token, match.group()))
                        i = match.end()
                        break
                if not match:
                    tokens.append(("class:text", line[i]))
                    i += 1

            return tokens

        return get_line_tokens
