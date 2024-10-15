import pytest
from goose.cli.prompt.overwrite_session_prompt import OverwriteSessionPrompt


@pytest.fixture
def prompt():
    return OverwriteSessionPrompt()


def test_init(prompt):
    assert prompt.choices == {
        "yes": "Overwrite the existing session",
        "no": "Pick a new session name",
        "resume": "Resume the existing session",
    }
    assert prompt.default == "resume"


@pytest.mark.parametrize(
    "choice, expected",
    [
        ("", False),
        ("invalid", False),
        ("n", True),
        ("N", True),
        ("no", True),
        ("NO", True),
        ("r", True),
        ("R", True),
        ("resume", True),
        ("RESUME", True),
        ("y", True),
        ("Y", True),
        ("yes", True),
        ("YES", True),
    ],
)
def test_check_choice(prompt, choice, expected):
    assert prompt.check_choice(choice) == expected


def test_instantiation():
    prompt = OverwriteSessionPrompt()
    assert prompt.choices == {
        "yes": "Overwrite the existing session",
        "no": "Pick a new session name",
        "resume": "Resume the existing session",
    }
    assert prompt.default == "resume"
