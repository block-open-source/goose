import pytest
from exchange.load_exchange_attribute_error import LoadExchangeAttributeError
from exchange.providers import get_provider


def test_get_provider_valid():
    provider_name = "openai"
    provider = get_provider(provider_name)
    assert provider.__name__ == "OpenAiProvider"


def test_get_provider_throw_error_for_unknown_provider():
    with pytest.raises(LoadExchangeAttributeError) as error:
        get_provider("nonexistent")
    assert error.value.attribute_name == "provider"
    assert error.value.attribute_value == "nonexistent"
    assert "openai" in error.value.available_values
    assert "openai" in error.value.message
