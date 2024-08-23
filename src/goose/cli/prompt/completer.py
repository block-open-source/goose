import re
from typing import List

from prompt_toolkit.completion import CompleteEvent, Completer, Completion
from prompt_toolkit.document import Document

from goose.command.base import Command


class GoosePromptCompleter(Completer):
    def __init__(self, commands: List[Command]) -> None:
        self.commands = commands

    def get_command_completions(self, document: Document) -> List[Completion]:
        all_completions = []
        for command_name, command_instance in self.commands.items():
            pattern = rf"(?<!\S)\/{command_name}:([\S]*)$"
            text = document.text_before_cursor
            match = re.search(pattern=pattern, string=text)
            if not match or text.endswith(" "):
                continue

            query = match.group(1)
            completions = command_instance.get_completions(query)
            all_completions.extend(completions)
        return all_completions

    def get_command_name_completions(self, document: Document) -> List[Completion]:
        pattern = r"(?<!\S)\/([\S]*)$"
        text = document.text_before_cursor
        match = re.search(pattern=pattern, string=text)
        if not match or text.endswith(" "):
            return []

        query = match.group(1)

        completions = []
        for command_name in self.commands:
            if command_name.startswith(query):
                completions.append(Completion(command_name, start_position=-len(query), display=command_name))
        return completions

    def get_completions(self, document: Document, _: CompleteEvent) -> List[Completion]:
        command_completions = self.get_command_completions(document)
        command_name_completions = self.get_command_name_completions(document)
        return command_name_completions + command_completions
