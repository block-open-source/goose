from datetime import datetime
import importlib
from time import time
from unittest.mock import MagicMock, patch

import click
import pytest
from click.testing import CliRunner
from exchange import Message
from goose.cli.main import cli, goose_cli


@pytest.fixture
def mock_print():
    with patch("goose.cli.main.print") as mock_print:
        yield mock_print


@pytest.fixture
def mock_session_files_path(tmp_path):
    with patch("goose.cli.main.SESSIONS_PATH", tmp_path) as session_files_path:
        yield session_files_path


@pytest.fixture
def mock_session():
    with patch("goose.cli.main.Session") as mock_session_class:
        mock_session_instance = MagicMock()
        mock_session_class.return_value = mock_session_instance
        yield mock_session_class, mock_session_instance


def test_session_resume_command_with_session_name(mock_session):
    mock_session_class, mock_session_instance = mock_session
    runner = CliRunner()
    runner.invoke(goose_cli, ["session", "resume", "session1", "--profile", "default"])
    mock_session_class.assert_called_once_with(name="session1", profile="default")
    mock_session_instance.run.assert_called_once()


def test_session_resume_command_without_session_name_without_session_files(
    mock_print, mock_session_files_path, mock_session
):
    _, mock_session_instance = mock_session
    runner = CliRunner()
    runner.invoke(goose_cli, ["session", "resume"])
    mock_print.assert_called_with("No sessions found.")
    mock_session_instance.run.assert_not_called()


def test_session_resume_command_without_session_name_use_latest_session(
    mock_print, mock_session_files_path, mock_session, create_session_file
):
    mock_session_class, mock_session_instance = mock_session
    for index, session_name in enumerate(["first", "second"]):
        create_session_file([Message.user("Hello1")], mock_session_files_path / f"{session_name}.jsonl", time() + index)
    runner = CliRunner()
    runner.invoke(goose_cli, ["session", "resume", "--profile", "default"])

    second_file_path = mock_session_files_path / "second.jsonl"
    mock_print.assert_called_once_with(f"Resuming most recent session: second from {second_file_path}")
    mock_session_class.assert_called_once_with(name="second", profile="default")
    mock_session_instance.run.assert_called_once()


def test_session_list_command(mock_print, mock_session_files_path, create_session_file):
    create_session_file([Message.user("Hello")], mock_session_files_path / "abc.jsonl")
    runner = CliRunner()
    runner.invoke(goose_cli, ["session", "list"])
    file_time = datetime.fromtimestamp(mock_session_files_path.stat().st_mtime).strftime("%Y-%m-%d %H:%M:%S")
    mock_print.assert_called_with(f"{file_time}    abc")


def test_session_clear_command(mock_session_files_path, create_session_file):
    for index, session_name in enumerate(["first", "second"]):
        create_session_file([Message.user("Hello1")], mock_session_files_path / f"{session_name}.jsonl", time() + index)
    runner = CliRunner()
    runner.invoke(goose_cli, ["session", "clear", "--keep", "1"])

    session_files = list(mock_session_files_path.glob("*.jsonl"))
    assert len(session_files) == 1
    assert session_files[0].stem == "second"


def test_combined_group_option():
    with patch("goose.utils.load_plugins") as mock_load_plugin:
        group_option_name = "--describe-commands"

        def option_callback(ctx, *_):
            click.echo("Option callback")
            ctx.exit()

        mock_group_options = {
            "option1": lambda: click.option(
                group_option_name,
                is_flag=True,
                callback=option_callback,
            ),
        }

        def side_effect_func(param):
            if param == "goose.cli.group_option":
                return mock_group_options
            elif param == "goose.cli.group":
                return {}

        mock_load_plugin.side_effect = side_effect_func

        # reload cli after mocking
        importlib.reload(importlib.import_module("goose.cli.main"))
        import goose.cli.main

        cli = goose.cli.main.cli

        runner = CliRunner()
        result = runner.invoke(cli, [group_option_name])
        assert result.exit_code == 0


def test_combined_group_commands(mock_session):
    mock_session_class, mock_session_instance = mock_session
    runner = CliRunner()
    runner.invoke(cli, ["session", "resume", "session1", "--profile", "default"])
    mock_session_class.assert_called_once_with(name="session1", profile="default")
    mock_session_instance.run.assert_called_once()
