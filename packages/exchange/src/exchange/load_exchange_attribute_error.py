from typing import List


class LoadExchangeAttributeError(Exception):
    def __init__(self, attribute_name: str, attribute_value: str, available_values: List[str]) -> None:
        self.attribute_name = attribute_name
        self.attribute_value = attribute_value
        self.available_values = available_values
        self.message = (
            f"Unknown {attribute_name}: {attribute_value}."
            + f" Available {attribute_name}s: {', '.join(available_values)}"
        )
        super().__init__(self.message)
