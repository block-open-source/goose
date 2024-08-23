from unittest.mock import Mock

import pytest
from goose.cli.prompt.completer import GoosePromptCompleter
from goose.command.base import Command
from prompt_toolkit.completion import Completion
from prompt_toolkit.document import Document

# Mock Command class
dummy_command = Mock(spec=Command)

dummy_command.get_completions = Mock(
    return_value=[
        Completion(text="completion1"),
        Completion(text="completion2"),
    ]
)

commands_list = {"test_command1": dummy_command, "test_command2": dummy_command}


@pytest.fixture
def completer():
    return GoosePromptCompleter(commands=commands_list)


def test_get_command_completions(completer):
    document = Document(text="/test_command1:input")
    completions = list(completer.get_command_completions(document))
    assert len(completions) == 2
    assert completions[0].text == "completion1"
    assert completions[1].text == "completion2"


def test_get_command_name_completions(completer):
    document = Document(text="/test")
    completions = list(completer.get_command_name_completions(document))
    print(completions)
    assert len(completions) == 2
    assert completions[0].text == "test_command1"
    assert completions[1].text == "test_command2"


def test_get_completions(completer):
    document = Document(text="/test_command1:input")
    completions = list(completer.get_completions(document, None))
    print(completions)
    assert len(completions) == 2
    assert completions[0].text == "completion1"
    assert completions[1].text == "completion2"
