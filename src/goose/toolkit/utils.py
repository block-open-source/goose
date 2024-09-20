from pathlib import Path
from typing import Optional, Dict

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


def find_last_task_group_index(input_str: str) -> int:
    lines = input_str.splitlines()
    last_group_start_index = -1
    current_group_start_index = -1

    for i, line in enumerate(lines):
        line = line.strip()
        if line.startswith("-"):
            # If this is the first line of a new group, mark its start
            if current_group_start_index == -1:
                current_group_start_index = i
        else:
            # If we encounter a non-hyphenated line and had a group, update last group start
            if current_group_start_index != -1:
                last_group_start_index = current_group_start_index
                current_group_start_index = -1  # Reset for potential future groups

    # If the input ended in a task group, update the last group index
    if current_group_start_index != -1:
        last_group_start_index = current_group_start_index
    return last_group_start_index


def parse_plan(input_plan_str: str) -> Dict:
    last_group_start_index = find_last_task_group_index(input_plan_str)
    if last_group_start_index == -1:
        return {"kickoff_message": input_plan_str, "tasks": []}

    kickoff_message_list = input_plan_str.splitlines()[:last_group_start_index]
    kickoff_message = "\n".join(kickoff_message_list).strip()
    tasks_list = input_plan_str.splitlines()[last_group_start_index:]
    tasks_list_output = [s[1:] for s in tasks_list if s.strip()]  # filter leading -
    return {"kickoff_message": kickoff_message, "tasks": tasks_list_output}
