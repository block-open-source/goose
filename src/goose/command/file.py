import os
from typing import List

from prompt_toolkit.completion import Completion

from goose.command.base import Command


class FileCommand(Command):
    def get_completions(self, query: str) -> List[Completion]:
        if query.startswith("/"):
            directory = os.path.dirname(query)
            search_term = os.path.basename(query)
        else:
            directory = os.path.join(os.getcwd(), os.path.dirname(query))
            search_term = os.path.basename(query)

        # if query is a file, don't show completions
        if os.path.isfile(directory):
            return []

        # Get the list of files in the directory
        options = []
        try:
            for file_name in os.listdir(directory):
                if file_name.startswith(search_term):
                    full_path = os.path.join(directory, file_name)
                    if os.path.isdir(full_path):
                        options.append(
                            dict(
                                display_text="  " + file_name,
                                insert_text=file_name,
                                is_dir=True,
                            )
                        )
                    else:
                        options.append(
                            dict(
                                display_text="  " + file_name,
                                insert_text=file_name + " ",
                                is_dir=False,
                            )
                        )
        except FileNotFoundError:
            return []

        completions = []
        options.sort(key=lambda x: (not x["is_dir"], x["insert_text"]), reverse=False)
        for option in options:
            completions.append(
                Completion(
                    option["insert_text"],
                    start_position=-len(search_term),
                    display=option["display_text"],
                )
            )
        return completions

    def execute(self, query: str) -> str | None:
        # GOOSE-TODO: return the query
        pass
