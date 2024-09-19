from goose.utils._cost_calculator import _calculate_cost, get_total_cost_message
from exchange.token_usage_collector import TokenUsage


def test_calculate_cost():
    cost = _calculate_cost(TokenUsage(model="gpt-4o", input_tokens=10000, output_tokens=600))
    assert cost == 0.059


def test_get_total_cost_message():
    message = get_total_cost_message(
        [
            TokenUsage(model="gpt-4o", input_tokens=10000, output_tokens=600),
            TokenUsage(model="gpt-4o-mini", input_tokens=3000000, output_tokens=4000000),
        ]
    )
    expected_message = (
        "Cost for TokenUsage(model='gpt-4o', input_tokens=10000, output_tokens=600): $0.06\n"
        + "Cost for TokenUsage(model='gpt-4o-mini', input_tokens=3000000, output_tokens=4000000)"
        + ": $2.85\nTotal cost: $2.91"
    )
    assert message == expected_message
