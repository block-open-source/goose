import os
from pathlib import Path
from typing import Dict, List

from exchange import Message
from goose.toolkit.base import Toolkit, tool
from goose.toolkit.utils import get_language, render_template
from rich.markdown import Markdown
from rich.prompt import Confirm
from prompt_toolkit.shortcuts import confirm
from rich.table import Table
from rich.text import Text
from rich.rule import Rule

from goose.utils.execute_shell import execute_shell
from goose.utils.path_file_output import show_diff
from goose.utils.safe_mode import is_in_safe_mode

RULESTYLE = "bold"
RULEPREFIX = f"[{RULESTYLE}]───[/] "

TASKS_WITH_EMOJI = {"planned": "⏳", "complete": "✅", "failed": "❌", "in-progress": "🕑", "cancelled": "🚫", "skipped": "⏩"}


def keep_unsafe_command_prompt(command: str) -> bool:
    command_text = Text(command, style="bold red")
    message = (
        Text("\nWe flagged the command: ") + command_text + Text(" as potentially unsafe, do you want to proceed?")
    )
    return Confirm.ask(message, default=True)


class Developer(Toolkit):
    """Provides a set of general purpose development capabilities

    The tools include plan management, a general purpose shell execution tool, and file operations.
    We also include some default shell strategies in the prompt, such as using ripgrep
    """

    def __init__(self, *args: object, **kwargs: Dict[str, object]) -> None:
        super().__init__(*args, **kwargs)
        self.timestamps: Dict[str, float] = {}

    def system(self) -> str:
        """Retrieve system configuration details for developer"""
        hints_path = Path(".goosehints")
        system_prompt = Message.load("prompts/developer.jinja").text
        home_hints_path = Path.home() / ".config/goose/.goosehints"
        hints = []
        if hints_path.is_file():
            hints.append(render_template(hints_path))
        if home_hints_path.is_file():
            hints.append(render_template(home_hints_path))
        if hints:
            hints_text = "\n".join(hints)
            system_prompt = f"{system_prompt}\n\nHints:\n{hints_text}"
        return system_prompt

    @tool
    def update_plan(self, tasks: List[dict]) -> List[dict]:
        """
        Update the plan by overwriting all current tasks

        This can be used to update the status of a task. This update will be
        shown to the user directly, you do not need to reiterate it
        If any of the task is marked as cancelled, mark the subsequent tasks as cancelled as well.
        If any of the task is marked as skipped, move to the next task

        Args:
            tasks (List(dict)): The list of tasks, where each task is a dictionary
                with a key for the task "description" and the task "status". The status
                MUST be one of "planned", "complete", "failed", "in-progress", "cancelled" or "skipped.

        """
        # Validate the status of each task to ensure it is one of the accepted values.
        for task in tasks:
            if task["status"] not in TASKS_WITH_EMOJI.keys():
                raise ValueError(f"Invalid task status: {task['status']}")

        # Create a table with columns for the index, description, and status of each task.
        table = Table(expand=True)
        table.add_column("#", justify="right", style="magenta")
        table.add_column("Task", justify="left")
        table.add_column("Status", justify="left")

        for i, entry in enumerate(tasks):
            table.add_row(str(i), entry["description"], TASKS_WITH_EMOJI[entry["status"]])

        # Log the table to display it directly to the user
        # `.log` method is used here to log the command execution in the application's UX
        self.notifier.log(table)

        # Return the tasks unchanged as the function's primary purpose is to update and display the task status.
        return tasks

    @tool
    def patch_file(self, path: str, before: str, after: str) -> str:
        """Patch the file at the specified by replacing before with after

        Before **must** be present exactly once in the file, so that it can safely
        be replaced with after.

        return a dictionary that with keys:
        1) result: include the output and error concatenated into a single string, as
        you would see from running on the command line
        2) task_status: a string indicating if the user chose not to execute the command.
        There will also be an indication of if the command succeeded, failed or skipped.

        Args:
            path (str): The path to the file, in the format "path/to/file.txt"
            before (str): The content that will be replaced
            after (str): The content it will be replaced with
        """
        _path = Path(path)

        content = _path.read_text()

        if content.count(before) > 1:
            raise ValueError("The before content is present multiple times in the file, be more specific.")
        if content.count(before) < 1:
            raise ValueError("The before content was not found in file, be careful that you recreate it exactly.")

        self.notifier.log(Rule(RULEPREFIX + path, style=RULESTYLE, align="left"))
        show_diff(path, before, after)
        to_path_file = True
        if is_in_safe_mode():
            self.notifier.stop()
            to_path_file = confirm("Would like to continue to make change?")
            self.notifier.start()
        if to_path_file:
            self.notifier.status(f"editing {path}")
            content = content.replace(before, after)
            _path.write_text(content)
            return "Successfully replaced before with after."
        else:
            return {"result": "User chooses not to make the change on this file. skip the change", "task_status": "skipped"}

    @tool
    def read_file(self, path: str) -> str:
        """Read the content of the file at path

        Args:
            path (str): The destination file path, in the format "path/to/file.txt"
        """
        language = get_language(path)
        content = Path(path).expanduser().read_text()
        self.notifier.log(Markdown(f"```\ncat {path}\n```"))
        # Record the last read timestamp
        self.timestamps[path] = os.path.getmtime(path)
        return f"```{language}\n{content}\n```"

    @tool
    def shell(self, command: str,  explanation: str, write: bool) -> str:
        """
        This will explain the user about the command to be executed.  If you are 100% sure the command won't change
        any resources, states or environment on my computer, please set write to false,
        otherwise set write to true. so that user can be asked whether they want to execute the command.
        If the user agreed to execute the command, please make sure to execute the command in the correct directory. 
        The function will
        return a dictionary that with keys:
        1) result: include the output and error concatenated into a single string, as
        you would see from running on the command line
        2) task_status: a string indicating if the user chose not to execute the command.
        There will also be an indication of if the command succeeded, failed or cancelled.

        Args:
            command (str): The shell command to run. It can support multiline statements
                if you need to run more than one at a time.  Please also include the directory that the command will be run in.
            explanation (str): A brief explanation of what the command does, the current state of the system
                the directory where the command is going to be executed, and what the expected outcome is.
            write (bool): If True, the user will be asked to confirm the command before it is executed
        """
        # Log the command being executed in a visually structured format (Markdown).
        self.notifier.log(Rule(RULEPREFIX + "shell", style=RULESTYLE, align="left"))
        self.notifier.log(Markdown(f"```bash\n{command}\n```"))
        self.notifier.log(Markdown(f"**Explanation:** {explanation}"))
        if not write:
            return execute_shell(command, notifier=self.notifier, exchange_view=self.exchange_view)
        to_execute = True
        if is_in_safe_mode():
            self.notifier.stop()
            to_execute = confirm("Would like to continue to execute this command?")
            self.notifier.start()
        if to_execute:
            return execute_shell(command, notifier=self.notifier, exchange_view=self.exchange_view)
        else:
            return {"result": "User chooses not to execution the command. Subsequent commands will be cancelled.", "task_status": "cancelled"}


    @tool
    def write_file(self, path: str, content: str) -> str:
        """
        Write a file at the specified path with the provided content. This will create any directories if they do not exist.
        The content will fully overwrite the existing file.

        Args:
            path (str): The destination file path, in the format "path/to/file.txt"
            content (str): The raw file content.
        """  # noqa: E501
        to_write_file = True
        self.notifier.log(Rule(RULEPREFIX + path, style=RULESTYLE, align="left"))
        before = ""
        if Path(path).exists():
            with open(path, 'r') as file:
                before = file.read()
        show_diff(path, before, content)
        if is_in_safe_mode():
            self.notifier.stop()
            to_write_file = confirm("Would like to continue to make change?")
            self.notifier.start()
        if not to_write_file:
            return {"result": "User chooses not to write to this file. skip the change", "task_status": "skipped"}
        self.notifier.status("writing file")
        # Log the content that will be written to the file
        # .log` method is used here to log the command execution in the application's UX
        # this method is dynamically attached to functions in the Goose framework

        _path = Path(path)
        if path in self.timestamps:
            last_read_timestamp = self.timestamps.get(path, 0.0)
            current_timestamp = os.path.getmtime(path)
            if current_timestamp > last_read_timestamp:
                raise RuntimeError(
                    f"File '{path}' has been modified since it was last read."
                    + " Read the file to incorporate changes or update your plan."
                )

        # Prepare the path and create any necessary parent directories
        _path.parent.mkdir(parents=True, exist_ok=True)

        # Write the content to the file
        _path.write_text(content)

        # Update the last read timestamp after writing to the file
        self.timestamps[path] = os.path.getmtime(path)

        # Return a success message
        return f"Successfully wrote to {path}"
