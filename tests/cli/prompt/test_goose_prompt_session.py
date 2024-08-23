from unittest.mock import patch

import pytest
from goose.cli.prompt.goose_prompt_session import GoosePromptSession
from goose.cli.prompt.user_input import PromptAction, UserInput


@pytest.fixture
def mock_prompt_session():
    with patch("prompt_toolkit.PromptSession") as mock_prompt_session:
        yield mock_prompt_session


def test_get_save_session_name(mock_prompt_session):
    mock_prompt_session.prompt.return_value = "my_session"
    goose_prompt_session = GoosePromptSession(mock_prompt_session)

    assert goose_prompt_session.get_save_session_name() == "my_session"


def test_get_user_input_to_continue(mock_prompt_session):
    mock_prompt_session.prompt.return_value = "input_value"
    goose_prompt_session = GoosePromptSession(mock_prompt_session)

    user_input = goose_prompt_session.get_user_input()

    assert user_input == UserInput(PromptAction.CONTINUE, "input_value")


@pytest.mark.parametrize("exit_input", ["exit", ":q"])
def test_get_user_input_to_exit(exit_input, mock_prompt_session):
    mock_prompt_session.prompt.return_value = exit_input
    goose_prompt_session = GoosePromptSession(mock_prompt_session)

    user_input = goose_prompt_session.get_user_input()

    assert user_input == UserInput(PromptAction.EXIT)


@pytest.mark.parametrize("error", [EOFError, KeyboardInterrupt])
def test_get_user_input_to_exit_when_error_occurs(error, mock_prompt_session):
    mock_prompt_session.prompt.side_effect = error
    goose_prompt_session = GoosePromptSession(mock_prompt_session)

    user_input = goose_prompt_session.get_user_input()

    assert user_input == UserInput(PromptAction.EXIT)
