from typing import Optional
from exchange.providers.base import Usage

PRICES = {
    "gpt-4o": (2.50, 10.00),
    "gpt-4o-2024-08-06": (2.50, 10.00),
    "gpt-4o-2024-05-13": (5.00, 15.00),
    "gpt-4o-mini": (0.150, 0.600),
    "gpt-4o-mini-2024-07-18": (0.150, 0.600),
    "o1-preview": (15.00, 60.00),
    "o1-mini": (3.00, 12.00),
    "claude-3-5-sonnet-20240620": (3.00, 15.00),
    "anthropic.claude-3-5-sonnet-20240620-v1:0": (3.00, 15.00),
    "claude-3-opus-20240229": (15.00, 75.00),
    "anthropic.claude-3-opus-20240229-v1:0": (15.00, 75.00),
    "claude-3-haiku-20240307": (0.25, 1.25),
    "anthropic.claude-3-haiku-20240307-v1:0": (0.25, 1.25),
}


def _calculate_cost(model: str, token_usage: Usage) -> Optional[float]:
    model_name = model.lower()
    if model_name in PRICES:
        input_token_price, output_token_price = PRICES[model_name]
        return (input_token_price * token_usage.input_tokens + output_token_price * token_usage.output_tokens) / 1000000
    return None


def get_total_cost_message(token_usages: dict[str, Usage]) -> str:
    total_cost = 0
    message = ""
    for model, token_usage in token_usages.items():
        cost = _calculate_cost(model, token_usage)
        if cost is not None:
            message += f"Cost for model {model} {str(token_usage)}: ${cost:.2f}\n"
            total_cost += cost
        else:
            message += f"Cost for model {model} {str(token_usage)}: Not available\n"
    message += f"Total cost: ${total_cost:.2f}"
    return message
