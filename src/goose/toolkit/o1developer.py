""" o1developer enhances the goose loop with o1

The outer loop of goose is still running the nominal model, such as 4o
That cannot currently use o1 directly because of lack of tool calling support.
But it is also very slow for small operations.

Instead, this toolkit calls out to o1 for two critical operations:
- create_plan
- edit_file

This means that the outer loop can do operations like forming and running
commands and checking off tasks. But o1 is repsonsible for creating the plan
and editing files, both of which benefit significantly from its performance.

To do this, we use a "pseudotool" helper, which asks o1 for a json block
and interprets it to call a function.
"""

import os
import platform

from exchange.tool import Tool
from exchange.moderators import PassiveModerator
from pathlib import Path
from subprocess import CompletedProcess, run
from typing import Dict, List, Optional, Tuple
from goose.toolkit.pseudotool import pseudotool
from goose.utils.check_shell_command import is_dangerous_command

from exchange import Message
from rich import box
from rich.markdown import Markdown
from rich.panel import Panel
from rich.prompt import Confirm
from rich.table import Table
from rich.text import Text

from goose.toolkit.base import Toolkit, tool
from goose.toolkit.utils import get_language, render_template


def system_hints():
    """Get hints about the current system"""
    return f"""
The current session is running on an active shell with the following details:
    cwd: {os.getcwd()}
    os: {platform.system()}
    shell: {os.environ.get("SHELL", "UNKNOWN")}
    user = {os.environ.get('USER', 'Unknown')}
"""


def keep_unsafe_command_prompt(command: str) -> bool:
    command_text = Text(command, style="bold red")
    message = (
        Text("\nWe flagged the command: ")
        + command_text
        + Text(" as potentially unsafe, do you want to proceed?")
    )
    return Confirm.ask(message, default=True)


class O1Developer(Toolkit):
    """Provides a set of general purpose development capabilities, with o1 for planning and edits

    The tools include plan management, a general purpose shell execution tool, and file operations.
    We also include some default shell strategies in the prompt, such as using ripgrep
    """

    def __init__(self, *args: list, **kwargs: dict) -> None:
        super().__init__(*args, **kwargs)
        self.timestamps: Dict[str, float] = {}
        self.tasks = []
        self.current_path = None
        hints_path = Path(".goosehints")
        self.hints = None
        if hints_path.is_file():
            self.hints = render_template(hints_path)

    def system(self) -> str:
        """Retrieve system configuration details for developer"""
        system_prompt = Message.load("prompts/developer.jinja").text
        if self.hints:
            system_prompt = f"{system_prompt}\n\nHints:\n{self.hints}"
        return system_prompt

    def _inner_plan(self, tasks: List[str]) -> List[dict]:
        """Create a plan to solve the inbound request

        Your plan must provide concrete details about how to solve the problem using
        a two tools: calling a shell command and editing files. Together these can solve
        just about an problem. Assume the shell is already active.

        The tasks should be specific about not just what to do but how to do it.

        You will frequently be working with an existing code base, and need to ensure that your
        additions fit into existing best practices and follow the current structure. To ensure they
        do, seek out references as part of your plan.

        To direct for a search for or within files, **always** recommend `ripgrep`, such as
        ```
        rg {pattern} # to search within files
        rg --files | rg {pattern} # to find a file by name
        ```

        To direct for file edits, briefly summarize the goal of the edit. Details will be figured
        out later. Your task lisk should include only one entry per file to edit for efficiency -
        get all changes needed in that one pass per file.

        Your task list should be as short as possible while still being complete and explicit

        Args:
            tasks (List(str)): The list of tasks, where each task is a string describing what should be done
        """
        # Create a table with columns for the index, description, and status of each task.
        self.tasks = []
        for i, entry in enumerate(tasks):
            self.tasks.append([i, entry, "planned"])

        self.log_plan()
        return self.tasks

    @tool
    def create_plan(self) -> List[dict]:
        """Create a plan for the current set of instructions

        You will then execute against the plan and mark task statuses
        with set_task_status. You can use this create_plan to redo the plan
        if enough has changed that it requires an update.
        """
        tool = Tool.from_function(self._inner_plan)
        exchange = self.exchange_view.processor.replace(
            model="o1-preview", moderator=PassiveModerator(), tools=[]
        )
        # remove the previous message which was a tool_use Assistant message
        exchange.messages.pop()
        exchange.add(Message.assistant(f"Let's create a plan"))
        exchange.add(Message.user("Please tell me the plan specification"))

        hints = (self.hints or "") + system_hints()
        tasks = pseudotool(exchange, tool, hints=self.hints)
        return tasks

    def log_plan(self):
        table = Table(expand=True)
        table.add_column("#", justify="right", style="magenta")
        table.add_column("Task", justify="left")
        table.add_column("Status", justify="left")
        emoji = {"complete": "✅", "failed": "❌", "planned": "⏳"}
        for task in self.tasks:
            i, description, status = task
            table.add_row(str(i), description, emoji[status])
        self.notifier.log(table)

    @tool
    def set_task_status(self, index: int, status: str):
        """Set the status of task in the plan

        Args:
            index (int): The index of the task
            status (str): The new status for the task. This **must** be either "complete" or "failed"
        """
        if status not in {"complete", "failed"}:
            raise ValueError(f"Invalid status provided: {status}")

        self.tasks[index][2] = status
        self.log_plan()

    @tool
    def edit_file(self, path: str) -> str:
        """Initiate an edit of the specified file

        This will return the resulting changes

        Args:
            path (str): The path to the file, in the format "path/to/file.txt"
        """
        self.current_path = path
        self.notifier.status(f"editing {self.current_path}")
        _path = Path(path)
        tool = Tool.from_function(self._inner_edit)
        exchange = self.exchange_view.processor.replace(
            model="o1-preview", moderator=PassiveModerator(), tools=[]
        )
        # remove the previous message which was a tool_use Assistant message
        exchange.messages.pop()
        exchange.add(Message.assistant(f"Let's edit the file at {path}"))
        if _path.exists():
            content = _path.read_text()
            exchange.add(
                Message.user(
                    f"Okay, the current content of the file is:\n\n```{content}```\n\nPlease reply with the edit specification"
                )
            )
        else:
            exchange.add(
                Message.user(
                    f"Okay, that file is currently empty. Please reply with the edit specfication."
                )
            )
        output = pseudotool(exchange, tool, hints=self.hints)
        return output

    def _inner_edit(
        self,
        patches: Optional[List[Tuple[str, str]]] = [],
        overwrite: Optional[str] = None,
    ):
        """Edit the file content described above using one of two methods.

        You can reply with a list of patches, or a full overwrite. Choose the method
        based on which one will require less total symbols to write down.

        A list of patches takes the format [[before, after], ...] and each pair will replace the content
        from before with after. E.g. [["def hello_world()", "def hello(recipent)"]] will
        replace the `hello_world` with `hello(recipient)`. To avoid ambiguity, the
        before string must be present **exactly once** in the original, or we will raise an error.

        A full overwrite will completely replace the file. That means you need to include verbatim
        what you are changes but also all original content that needs to remain unchanged. If the file
        is empty always use overwrite.

        Args:
          patches (Optional[List[List[str]]]): A list of string before/after pairs. Each before will be replaced
            with the corresponding after
          overwrite (Optional[str]): A string to fully overwrite the file with
        """
        _path = Path(self.current_path)
        language = get_language(self.current_path)

        if overwrite and patches:
            raise ValueError("You should only specify one of overwrite or patches")
        if not overwrite and not patches:
            raise ValueError(
                "You must specify one of overwrite or patches, cannot omit both"
            )
        if not _path.exists() and not overwrite:
            raise ValueError(
                f"{self.current_path} is currently empty, you need to use overwrite"
            )

        if overwrite:
            content = overwrite
        elif _path.exists():
            content = _path.read_text()
            for patch in patches:
                before, after = patch
                if content.count(before) > 1:
                    raise ValueError(
                        "The before content is present multiple times in the file, be more specific."
                    )
                if content.count(before) < 1:
                    raise ValueError(
                        "The before content was not found in file, be careful that you recreate it exactly."
                    )
                content = content.replace(before, after)

        _path.parent.mkdir(parents=True, exist_ok=True)
        _path.write_text(content)

        language = get_language(self.current_path)
        md = f"```{language}\n{content}\n```"
        self.notifier.log(Panel.fit(Markdown(md), title=self.current_path))
        return f"File succesfully updated, the new content is:\n\n```{content}```"

    @tool
    def shell(self, command: str) -> str:
        """
        Execute a command on the shell (in OSX)

        This will return the output and error concatenated into a single string, as
        you would see from running on the command line. There will also be an indication
        of if the command succeeded or failed.

        Args:
            command (str): The shell command to run. It can support multiline statements
                if you need to run more than one at a time
        """
        self.notifier.status("planning to run shell command")
        # Log the command being executed in a visually structured format (Markdown).
        # The `.log` method is used here to log the command execution in the application's UX
        # this method is dynamically attached to functions in the Goose framework to handle user-visible
        # logging and integrates with the overall UI logging system
        self.notifier.log(
            Panel.fit(Markdown(f"```bash\n{command}\n```"), title="shell")
        )

        if is_dangerous_command(command):
            # Stop the notifications so we can prompt
            self.notifier.stop()
            if not keep_unsafe_command_prompt(command):
                raise RuntimeError(
                    f"The command {command} was rejected as dangerous by the user."
                    + " Do not proceed further, instead ask for instructions."
                )
            self.notifier.start()
        self.notifier.status("running shell command")
        result: CompletedProcess = run(
            command, shell=True, text=True, capture_output=True, check=False
        )
        if result.returncode == 0:
            output = "Command succeeded"
        else:
            output = f"Command failed with returncode {result.returncode}"
        return "\n".join([output, result.stdout, result.stderr])
