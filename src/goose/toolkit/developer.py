import os
import re
import subprocess
import time
from pathlib import Path
from typing import Dict, List

from exchange import Message
from goose.toolkit.base import Toolkit, tool
from goose.toolkit.utils import get_language, render_template
from goose.utils.ask import ask_an_ai
from goose.utils.check_shell_command import is_dangerous_command
from rich.markdown import Markdown
from rich.prompt import Confirm
from rich.table import Table
from rich.text import Text
from rich.rule import Rule

RULESTYLE = "bold"
RULEPREFIX = f"[{RULESTYLE}]───[/] "


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

        Args:
            tasks (List(dict)): The list of tasks, where each task is a dictionary
                with a key for the task "description" and the task "status". The status
                MUST be one of "planned", "complete", "failed", "in-progress".

        """
        # Validate the status of each task to ensure it is one of the accepted values.
        for task in tasks:
            if task["status"] not in {"planned", "complete", "failed", "in-progress"}:
                raise ValueError(f"Invalid task status: {task['status']}")

        # Create a table with columns for the index, description, and status of each task.
        table = Table(expand=True)
        table.add_column("#", justify="right", style="magenta")
        table.add_column("Task", justify="left")
        table.add_column("Status", justify="left")

        # Mapping of statuses to emojis for better visual representation in the table.
        emoji = {"planned": "⏳", "complete": "✅", "failed": "❌", "in-progress": "🕑"}
        for i, entry in enumerate(tasks):
            table.add_row(str(i), entry["description"], emoji[entry["status"]])

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

        Args:
            path (str): The path to the file, in the format "path/to/file.txt"
            before (str): The content that will be replaced
            after (str): The content it will be replaced with
        """
        self.notifier.status(f"editing {path}")
        _path = Path(path)
        language = get_language(path)

        content = _path.read_text()

        if content.count(before) > 1:
            raise ValueError("The before content is present multiple times in the file, be more specific.")
        if content.count(before) < 1:
            raise ValueError("The before content was not found in file, be careful that you recreate it exactly.")

        content = content.replace(before, after)
        _path.write_text(content)

        output = f"""
```{language}
{before}
```
->
```{language}
{after}
```
"""
        self.notifier.log(Rule(RULEPREFIX + path, style=RULESTYLE, align="left"))
        self.notifier.log(Markdown(output))
        return "Succesfully replaced before with after."

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
    def shell(self, command: str) -> str:
        """
        Execute a command on the shell

        This will return the output and error concatenated into a single string, as
        you would see from running on the command line. There will also be an indication
        of if the command succeeded or failed.

        Args:
            command (str): The shell command to run. It can support multiline statements
                if you need to run more than one at a time
        """
        # Log the command being executed in a visually structured format (Markdown).
        self.notifier.log(Rule(RULEPREFIX + "shell", style=RULESTYLE, align="left"))
        self.notifier.log(Markdown(f"```bash\n{command}\n```"))

        if is_dangerous_command(command):
            # Stop the notifications so we can prompt
            self.notifier.stop()
            if not keep_unsafe_command_prompt(command):
                raise RuntimeError(
                    f"The command {command} was rejected as dangerous by the user."
                    " Do not proceed further, instead ask for instructions."
                )
            self.notifier.start()
        self.notifier.status("running shell command")

        # Define patterns that might indicate the process is waiting for input
        interaction_patterns = [
            r"Do you want to",  # Common prompt phrase
            r"Enter password",  # Password prompt
            r"Are you sure",  # Confirmation prompt
            r"\(y/N\)",  # Yes/No prompt
            r"Press any key to continue",  # Awaiting keypress
            r"Waiting for input",  # General waiting message
        ]
        compiled_patterns = [re.compile(pattern, re.IGNORECASE) for pattern in interaction_patterns]

        proc = subprocess.Popen(
            command,
            shell=True,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
        )
        # this enables us to read lines without blocking
        os.set_blocking(proc.stdout.fileno(), False)

        # Accumulate the output logs while checking if it might be blocked
        output_lines = []
        last_output_time = time.time()
        cutoff = 10
        while proc.poll() is None:
            self.notifier.status("running shell command")
            line = proc.stdout.readline()
            if line:
                output_lines.append(line)
                last_output_time = time.time()

            # If we see a clear pattern match, we plan to abort
            exit_criteria = any(pattern.search(line) for pattern in compiled_patterns)

            # and if we haven't seen a new line in 10+s, check with AI to see if it may be stuck
            if not exit_criteria and time.time() - last_output_time > cutoff:
                self.notifier.status("checking on shell status")
                response = ask_an_ai(
                    input="\n".join([command] + output_lines),
                    prompt=(
                        "You will evaluate the output of shell commands to see if they may be stuck."
                        " Look for commands that appear to be awaiting user input, or otherwise running indefinitely (such as a web service)."  # noqa
                        " A command that will take a while, such as downloading resources is okay."  # noqa
                        " return [Yes] if stuck, [No] otherwise."
                    ),
                    exchange=self.exchange_view.processor,
                    with_tools=False,
                )
                exit_criteria = "[yes]" in response.content[0].text.lower()
                # We add exponential backoff for how often we check for the command being stuck
                cutoff *= 10

            if exit_criteria:
                proc.terminate()
                raise ValueError(
                    f"The command `{command}` looks like it will run indefinitely or is otherwise stuck."
                    f"You may be able to specify inputs if it applies to this command."
                    f"Otherwise to enable continued iteration, you'll need to ask the user to run this command in another terminal."  # noqa
                )

        # read any remaining lines
        while line := proc.stdout.readline():
            output_lines.append(line)
        output = "".join(output_lines)

        # Determine the result based on the return code
        if proc.returncode == 0:
            result = "Command succeeded"
        else:
            result = f"Command failed with returncode {proc.returncode}"

        # Return the combined result and outputs if we made it this far
        return "\n".join([result, output])

    @tool
    def write_file(self, path: str, content: str) -> str:
        """
        Write a file at the specified path with the provided content. This will create any directories if they do not exist.
        The content will fully overwrite the existing file.

        Args:
            path (str): The destination file path, in the format "path/to/file.txt"
            content (str): The raw file content.
        """  # noqa: E501
        self.notifier.status("writing file")
        # Get the programming language for syntax highlighting in logs
        language = get_language(path)
        md = f"```{language}\n{content}\n```"

        # Log the content that will be written to the file
        # .log` method is used here to log the command execution in the application's UX
        # this method is dynamically attached to functions in the Goose framework
        self.notifier.log(Rule(RULEPREFIX + path, style=RULESTYLE, align="left"))
        self.notifier.log(Markdown(md))

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
