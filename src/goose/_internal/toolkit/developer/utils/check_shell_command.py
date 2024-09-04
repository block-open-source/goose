import re


def is_dangerous_command(command: str) -> bool:
    """
    Check if the command matches any dangerous patterns.

    Args:
        command (str): The shell command to check.

    Returns:
        bool: True if the command is dangerous, False otherwise.
    """
    dangerous_patterns = [
        r"\brm\b",  # rm command
        r"\bgit\s+push\b",  # git push command
        r"\bsudo\b",  # sudo command
        # Add more dangerous command patterns here
        r"\bmv\b",  # mv command
        r"\bchmod\b",  # chmod command
        r"\bchown\b",  # chown command
        r"\bmkfs\b",  # mkfs command
        r"\bsystemctl\b",  # systemctl command
        r"\breboot\b",  # reboot command
        r"\bshutdown\b",  # shutdown command
        # Manipulating files in ~/ directly or dot files
        r"^~/[^/]+$",  # Files directly in home directory
        r"/\.[^/]+$",  # Dot files
    ]
    for pattern in dangerous_patterns:
        if re.search(pattern, command):
            return True
    return False
