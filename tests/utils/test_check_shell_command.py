import pytest
from goose.utils.check_shell_command import is_dangerous_command


@pytest.mark.parametrize(
    "command",
    [
        "rm -rf /",
        "git push origin master",
        "sudo reboot",
        "mv /etc/passwd /tmp/",
        "chmod 777 /etc/passwd",
        "chown root:root /etc/passwd",
        "mkfs -t ext4 /dev/sda1",
        "systemctl stop nginx",
        "reboot",
        "shutdown now",
        "echo hello > ~/.bashrc",
    ],
)
def test_dangerous_commands(command):
    assert is_dangerous_command(command)


@pytest.mark.parametrize("command", ["ls -la", 'echo "Hello World"', "cp ~/folder/file.txt /tmp/"])
def test_safe_commands(command):
    assert not is_dangerous_command(command)
