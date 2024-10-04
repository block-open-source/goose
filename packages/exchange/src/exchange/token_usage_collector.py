from collections import defaultdict
from typing import Dict

from exchange.providers.base import Usage


class _TokenUsageCollector:
    def __init__(self) -> None:
        self.usage_data = []

    def collect(self, model: str, usage: Usage) -> None:
        self.usage_data.append((model, usage))

    def get_token_usage_group_by_model(self) -> Dict[str, Usage]:
        usage_group_by_model = defaultdict(lambda: Usage(0, 0, 0))
        for model, usage in self.usage_data:
            usage_by_model = usage_group_by_model[model]
            if usage is not None and usage.input_tokens is not None:
                usage_by_model.input_tokens += usage.input_tokens
            if usage is not None and usage.output_tokens is not None:
                usage_by_model.output_tokens += usage.output_tokens
            if usage is not None and usage.total_tokens is not None:
                usage_by_model.total_tokens += usage.total_tokens
        return usage_group_by_model


_token_usage_collector = _TokenUsageCollector()
