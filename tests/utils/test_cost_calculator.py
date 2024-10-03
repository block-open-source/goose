from unittest.mock import patch
import pytest
from goose.utils._cost_calculator import _calculate_cost, get_total_cost_message
from exchange.providers.base import Usage


@pytest.fixture
def mock_prices():
    prices = {"gpt-4o": (5.00, 15.00), "gpt-4o-mini": (0.150, 0.600)}
    with patch("goose.utils._cost_calculator.PRICES", prices) as mock_prices:
        yield mock_prices


def test_calculate_cost(mock_prices):
    cost = _calculate_cost("gpt-4o", Usage(input_tokens=10000, output_tokens=600, total_tokens=10600))
    assert cost == 0.059


def test_get_total_cost_message(mock_prices):
    message = get_total_cost_message(
        {
            "gpt-4o": Usage(input_tokens=10000, output_tokens=600, total_tokens=10600),
            "gpt-4o-mini": Usage(input_tokens=3000000, output_tokens=4000000, total_tokens=7000000),
        }
    )
    expected_message = (
        "Cost for model gpt-4o Usage(input_tokens=10000, output_tokens=600, total_tokens=10600): $0.06\n"
        + "Cost for model gpt-4o-mini Usage(input_tokens=3000000, output_tokens=4000000, total_tokens=7000000)"
        + ": $2.85\nTotal cost: $2.91"
    )
    assert message == expected_message


def test_get_total_cost_message_with_non_available_pricing(mock_prices):
    message = get_total_cost_message(
        {
            "non_pricing_model": Usage(input_tokens=10000, output_tokens=600, total_tokens=10600),
            "gpt-4o-mini": Usage(input_tokens=3000000, output_tokens=4000000, total_tokens=7000000),
        }
    )
    expected_message = (
        "Cost for model non_pricing_model Usage(input_tokens=10000, output_tokens=600, total_tokens=10600): "
        + "Not available\n"
        + "Cost for model gpt-4o-mini Usage(input_tokens=3000000, output_tokens=4000000, total_tokens=7000000)"
        + ": $2.85\nTotal cost: $2.85"
    )
    assert message == expected_message
