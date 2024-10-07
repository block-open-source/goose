import difflib
from rich.text import Text
from rich.rule import Rule
from rich import print


def show_diff(file_name: str, before: str, after: str) -> str:
    before_lines = before.splitlines()
    after_lines = after.splitlines()

    diff = difflib.unified_diff(
        before_lines, after_lines,
        fromfile=f"{file_name} (before)",
        tofile=f"{file_name} (after)",
        lineterm=""
    )
    for line in diff:
        if line.startswith('---') or line.startswith('+++'):
            # Style the file changes
            print(Text(line, style="bold blue"))
        elif line.startswith('@@'):
            # Style the diff position
            print(Text(line, style="bold yellow"))
        elif line.startswith('-'):
            # Style removed lines
            print(Text(line, style="red"))
        elif line.startswith('+'):
            # Style added lines
            print(Text(line, style="green"))
        else:
            # Style unchanged lines
            print(Text(line, style="dim"))
