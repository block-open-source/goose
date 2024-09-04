import re
from typing import Callable, List, Tuple

from prompt_toolkit.document import Document
from prompt_toolkit.lexers import Lexer


def completion_for_command(target_string: str) -> re.Pattern[str]:
    escaped_string = re.escape(target_string)
    vals = [f"(?:{escaped_string[:i]}(?=$))" for i in range(len(escaped_string), 0, -1)]
    return re.compile(rf'(?<!\S)\/({"|".join(vals)})(?:\s^|$)')


def command_itself(target_string: str) -> re.Pattern[str]:
    escaped_string = re.escape(target_string)
    return re.compile(rf"(?<!\S)(\/{escaped_string})")


def value_for_command(command_string: str) -> re.Pattern[str]:
    escaped_string = re.escape(command_string)
    return re.compile(rf"(?<=(?<!\S)\/{escaped_string})([^\s]*)")


class PromptLexer(Lexer):
    def __init__(self, command_names: List[str]) -> None:
        self.patterns = []
        for command_name in command_names:
            full_command = command_name + ":"
            self.patterns.append((completion_for_command(full_command), "class:command"))
            self.patterns.append((value_for_command(full_command), "class:parameter"))
            self.patterns.append((command_itself(full_command), "class:command"))

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
