from unittest.mock import MagicMock, patch

import pytest
from goose.cli.prompt.prompt_validator import PromptValidator
from prompt_toolkit.validation import ValidationError


@pytest.fixture
def validator():
    return PromptValidator()


@patch("prompt_toolkit.document.Document.text")
def test_validate_should_not_raise_error_when_input_is_none(document, validator):
    try:
        validator.validate(create_mock_document(None))
    except Exception as e:
        pytest.fail(f"An error was raised: {e}")


@patch("prompt_toolkit.document.Document.text", return_value="user typed something")
def test_validate_should_not_raise_error_when_user_has_input(document, validator):
    try:
        validator.validate(create_mock_document("user typed something"))
    except Exception as e:
        pytest.fail(f"An error was raised: {e}")


def test_validate_should_raise_validation_error_when_user_has_empty_input(validator):
    with pytest.raises(ValidationError):
        validator.validate(create_mock_document(""))


def create_mock_document(text: str) -> MagicMock:
    document = MagicMock()
    document.text = text
    return document
