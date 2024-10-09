from exchange.invalid_choice_error import InvalidChoiceError


def test_load_invalid_choice_error():
    attribute_name = "moderator"
    attribute_value = "not_exist"
    available_values = ["truncate", "summarizer"]
    error = InvalidChoiceError(attribute_name, attribute_value, available_values)

    assert error.attribute_name == attribute_name
    assert error.attribute_value == attribute_value
    assert error.attribute_value == attribute_value
    assert error.message == "Unknown moderator: not_exist. Available moderators: truncate, summarizer"
