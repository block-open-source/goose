import os
import re
import subprocess
import time
from typing import Mapping, Optional

from goose.notifier import Notifier
from goose.utils.ask import ask_an_ai
from goose.view import ExchangeView
from rich.prompt import Confirm


def is_dangerous_command(command: str) -> bool:
    """
    Check if the command matches any dangerous patterns.

    Dangerous patterns in this function are defined as commands that may present risk to system stability.

    Args:
        command (str): The shell command to check.

    Returns:
        bool: True if the command is dangerous, False otherwise.
    """
    dangerous_patterns = [
        # Commands that are generally unsafe
        r"\brm\b",  # rm command
        r"\bgit\s+push\b",  # git push command
        r"\bsudo\b",  # sudo command
        r"\bmv\b",  # mv command
        r"\bchmod\b",  # chmod command
        r"\bchown\b",  # chown command
        r"\bmkfs\b",  # mkfs command
        r"\bsystemctl\b",  # systemctl command
        r"\breboot\b",  # reboot command
        r"\bshutdown\b",  # shutdown command
        # Target files that are unsafe
        r"\b~\/\.|\/\.\w+",  # commands that point to files or dirs in home that start with a dot (dotfiles)
    ]
    for pattern in dangerous_patterns:
        if re.search(pattern, command):
            return True
    return False


def keep_unsafe_command_prompt(command: str) -> bool:
    message = f"\nWe flagged the command - [bold red]{command}[/] - as potentially unsafe, do you want to proceed?"
    return Confirm.ask(message, default=True)


def shell(
    command: str,
    notifier: Notifier,
    exchange_view: ExchangeView,
    cwd: Optional[str] = None,
    env: Optional[Mapping[str, str]] = None,
) -> str:
    """Execute a command on the shell

    This handles
    """
    if is_dangerous_command(command):
        # Stop the notifications so we can prompt
        notifier.stop()
        if not keep_unsafe_command_prompt(command):
            raise RuntimeError(
                f"The command {command} was rejected as dangerous by the user."
                " Do not proceed further, instead ask for instructions."
            )
        notifier.start()
    notifier.status("running shell command")

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
        cwd=cwd,
        env=env,
    )
    # this enables us to read lines without blocking
    os.set_blocking(proc.stdout.fileno(), False)

    # Accumulate the output logs while checking if it might be blocked
    output_lines = []
    last_output_time = time.time()
    cutoff = 10
    while proc.poll() is None:
        notifier.status("running shell command")
        line = proc.stdout.readline()
        if line:
            output_lines.append(line)
            last_output_time = time.time()

        # If we see a clear pattern match, we plan to abort
        exit_criteria = any(pattern.search(line) for pattern in compiled_patterns)

        # and if we haven't seen a new line in 10+s, check with AI to see if it may be stuck
        if not exit_criteria and time.time() - last_output_time > cutoff:
            notifier.status("checking on shell status")
            response = ask_an_ai(
                input="\n".join([command] + output_lines),
                prompt=(
                    "You will evaluate the output of shell commands to see if they may be stuck."
                    " Look for commands that appear to be awaiting user input, or otherwise running indefinitely (such as a web service)."  # noqa
                    " A command that will take a while, such as downloading resources is okay."  # noqa
                    " return [Yes] if stuck, [No] otherwise."
                ),
                exchange=exchange_view.processor,
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
