from exchange.invalid_choice_error import InvalidChoiceError
from exchange.moderators import get_moderator
import pytest


def test_get_moderator():
    moderator = get_moderator("truncate")
    assert moderator.__name__ == "ContextTruncate"


def test_get_moderator_raise_error_for_unknown_moderator():
    with pytest.raises(InvalidChoiceError) as error:
        get_moderator("nonexistent")
    assert error.value.attribute_name == "moderator"
    assert error.value.attribute_value == "nonexistent"
    assert "truncate" in error.value.available_values
    assert "truncate" in error.value.message
