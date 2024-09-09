import re


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
