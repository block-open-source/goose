from click.testing import CliRunner
from goose.cli.main import get_current_shell, shell_completions


def test_get_current_shell(mocker):
    mocker.patch("os.getenv", return_value="/bin/bash")
    assert get_current_shell() == "bash"


def test_shell_completions_install_invalid_combination():
    runner = CliRunner()
    result = runner.invoke(shell_completions, ["--install", "--generate", "bash"])
    assert result.exit_code != 0
    assert "Only one of --install or --generate can be specified" in result.output


def test_shell_completions_install_no_option():
    runner = CliRunner()
    result = runner.invoke(shell_completions, ["bash"])
    assert result.exit_code != 0
    assert "One of --install or --generate must be specified" in result.output
