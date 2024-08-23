from pathlib import Path

from pygments.lexers import get_lexer_for_filename
from pygments.util import ClassNotFound


def get_language(filename: Path) -> str:
    """
    Determine the programming language of a file based on its filename extension.

    Args:
        filename (str): The name of the file for which to determine the programming language.

    Returns:
        str: The name of the programming language if recognized, otherwise an empty string.
    """
    try:
        lexer = get_lexer_for_filename(filename)
        return lexer.name
    except ClassNotFound:
        return ""
