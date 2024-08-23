from typing import List

from rich.text import Text


def diff(a: str, b: str) -> List[str]:
    """Returns a string containing the unified diff of two strings."""

    import difflib

    a_lines = a.splitlines()
    b_lines = b.splitlines()

    # Create a Differ object
    d = difflib.Differ()

    # Generate the diff
    diff = list(d.compare(a_lines, b_lines))

    return diff


def pretty_diff(a: str, b: str) -> Text:
    """Returns a pretty-printed diff of two strings."""

    diff_lines = diff(a, b)
    result = Text()
    for line in diff_lines:
        if line.startswith("+"):
            result.append(line, style="green")
        elif line.startswith("-"):
            result.append(line, style="red")
        elif line.startswith("?"):
            result.append(line, style="yellow")
        else:
            result.append(line, style="dim grey")
        result.append("\n")

    return result
