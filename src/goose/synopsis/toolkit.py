# janky global state for now, think about it
import subprocess
import os
from pathlib import Path
from typing import Dict

from exchange import Message
from goose.synopsis.system import system
from goose.toolkit.base import Toolkit, tool
from goose.toolkit.utils import RULEPREFIX, RULESTYLE, get_language
from goose.utils.shell import is_dangerous_command, shell, keep_unsafe_command_prompt
from rich.markdown import Markdown
from rich.rule import Rule


class SynopsisDeveloper(Toolkit):
    """Provides shell and file operation tools using OperatingSystem."""

    def __init__(self, *args: object, **kwargs: Dict[str, object]) -> None:
        super().__init__(*args, **kwargs)

    def system(self) -> str:
        """Retrieve system configuration details for developer"""
        system_prompt = Message.load("developer.md").text
        return system_prompt

    def logshell(self, command: str, title: str = "shell") -> None:
        self.notifier.log("")
        self.notifier.log(
            Rule(RULEPREFIX + f"{title} | [dim magenta]{os.path.abspath(system.cwd)}[/]", style=RULESTYLE, align="left")
        )
        self.notifier.log(Markdown(f"```bash\n{command}\n```"))
        self.notifier.log("")

    @tool
    def source(self, path: str) -> str:
        """Source the file at path, keeping the updates reflected in future shell commands

        Args:
            path (str): The path to the file to source.
        """
        source_command = f"source {path} && env"
        self.logshell(f"source {path}")
        result = shell(source_command, self.notifier, self.exchange_view, cwd=system.cwd, env=system.env)
        env_vars = dict(line.split("=", 1) for line in result.splitlines() if "=" in line)
        system.env.update(env_vars)
        return f"Sourced {path}"

    @tool
    def shell(self, command: str) -> str:
        """Execute any command on the shell

        Args:
            command (str): The shell command to run. It can support multiline statements
                if you need to run more than one at a time
        """
        if command.startswith("cat"):
            raise ValueError("You must read files through the read_file tool.")
        if command.startswith("cd"):
            raise ValueError("You must change dirs through the change_dir tool.")
        if command.startswith("source"):
            raise ValueError("You must source files through the source tool.")

        self.logshell(command)
        return shell(command, self.notifier, self.exchange_view, cwd=system.cwd, env=system.env)

    @tool
    def read_file(self, path: str) -> str:
        """Read the content of the file at path

        Args:
            path (str): The destination file path, in the format "path/to/file.txt"
        """
        system.remember_file(path)
        self.logshell(f"cat {path}")
        return f"The file content at {path} has been updated above."

    @tool
    def write_file(self, path: str, content: str) -> str:
        """
        Write a file at the specified path with the provided content. This will create any directories if they do not exist.
        The content will fully overwrite the existing file.

        Args:
            path (str): The destination file path, in the format "path/to/file.txt"
            content (str): The raw file content.
        """  # noqa: E501
        patho = system.to_patho(path)

        if patho.exists() and not system.is_active(path):
            print(f"We are warning the LLM to view before write in write_file, with path={path} and patho={str(patho)}")
            raise ValueError(f"You must view {path} using read_file before you overwrite it")

        patho.parent.mkdir(parents=True, exist_ok=True)
        patho.write_text(content)
        system.remember_file(path)

        language = get_language(path)
        md = f"```{language}\n{content}\n```"

        self.notifier.log("")
        self.notifier.log(Rule(RULEPREFIX + path, style=RULESTYLE, align="left"))
        self.notifier.log(Markdown(md))
        self.notifier.log("")

        return f"Successfully wrote to {path}"

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
        patho = system.to_patho(path)

        if not patho.exists():
            raise ValueError(f"You can't patch {path} - it does not exist yet")

        if not system.is_active(path):
            raise ValueError(f"You must view {path} using read_file before you patch it")

        language = get_language(path)

        content = patho.read_text()

        if content.count(before) > 1:
            raise ValueError("The before content is present multiple times in the file, be more specific.")
        if content.count(before) < 1:
            raise ValueError("The before content was not found in file, be careful that you recreate it exactly.")

        content = content.replace(before, after)
        system.remember_file(path)
        patho.write_text(content)

        output = f"""
```{language}
{before}
```
->
```{language}
{after}
```
"""
        self.notifier.log("")
        self.notifier.log(Rule(RULEPREFIX + path, style=RULESTYLE, align="left"))
        self.notifier.log(Markdown(output))
        self.notifier.log("")
        return "Succesfully replaced before with after."

    @tool
    def start_process(self, command: str) -> int:
        """Start a background process running the specified command

        Use this exclusively for processes that you need to run in the background
        because they do not terminate, such as running a webserver.

        Args:
            command (str): The shell command to run
        """
        self.logshell(command, title="background")

        if is_dangerous_command(command):
            self.notifier.stop()
            if not keep_unsafe_command_prompt(command):
                raise RuntimeError(
                    f"The command {command} was rejected as dangerous by the user."
                    " Do not proceed further, instead ask for instructions."
                )
            self.notifier.start()

        process = subprocess.Popen(
            command,
            shell=True,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            cwd=system.cwd,
            env=system.env,
        )
        process_id = system.add_process(process)
        return process_id

    @tool
    def list_processes(self) -> Dict[int, str]:
        """List all running background processes with their IDs and commands."""
        processes = system.get_processes()
        process_list = "```\n" + "\n".join(f"id: {pid}, command: {cmd}" for pid, cmd in processes.items()) + "\n```"
        self.notifier.log("")
        self.notifier.log(Rule(RULEPREFIX + "processes", style=RULESTYLE, align="left"))
        self.notifier.log(Markdown(process_list))
        self.notifier.log("")
        return processes

    @tool
    def view_process_output(self, process_id: int) -> str:
        """View the output of a running background process

        Args:
            process_id (int): The ID of the process to view output.
        """
        self.notifier.log("")
        self.notifier.log(Rule(RULEPREFIX + "processes", style=RULESTYLE, align="left"))
        self.notifier.log(Markdown(f"```\nreading {process_id}\n```"))
        self.notifier.log("")
        output = system.view_process_output(process_id)
        return output

    @tool
    def cancel_process(self, process_id: int) -> str:
        """Cancel the background process with the specified ID.

        Args:
            process_id (int): The ID of the process to be cancelled.
        """
        result = system.cancel_process(process_id)
        self.logshell(f"kill {process_id}")
        if result:
            return f"process {process_id} cancelled"
        else:
            return f"no known process {process_id}"

    @tool
    def change_dir(self, path: str) -> str:
        """Change the directory to the specified path

        Args:
            path (str): The new dir path, in the format "path/to/dir"
        """
        patho = system.to_patho(path)
        if not patho.is_dir():
            raise ValueError(f"The directory {path} does not exist")
        if patho.resolve() < Path(os.getcwd()).resolve():
            raise ValueError("You can cd into subdirs but not above the directory where we started.")
        self.logshell(f"cd {path}")
        system.cwd = str(patho)
        return path
