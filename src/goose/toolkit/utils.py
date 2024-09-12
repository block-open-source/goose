from pathlib import Path
from typing import Optional

from pygments.lexers import get_lexer_for_filename
from pygments.util import ClassNotFound

from jinja2 import Environment, FileSystemLoader


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


def render_template(template_path: Path, context: Optional[dict] = None) -> str:
    """
    Renders a Jinja2 template given a Pathlib path, with no context needed.

    :param template_path: Path to the Jinja2 template file.
    :param context: Optional dictionary containing the context for rendering the template.
    :return: Rendered template as a string.
    """
    # Ensure the path is absolute and exists
    if not template_path.is_absolute():
        template_path = template_path.resolve()

    if not template_path.exists():
        raise FileNotFoundError(f"Template file {template_path} does not exist.")

    env = Environment(loader=FileSystemLoader(template_path.parent))
    template = env.get_template(template_path.name)
    return template.render(context or {})
