from functools import cache
from exchange.load_exchange_attribute_error import LoadExchangeAttributeError
from goose.toolkit.base import Toolkit
from goose.utils import load_plugins


@cache
def get_toolkit(name: str) -> type[Toolkit]:
    toolkits = load_plugins(group="goose.toolkit")
    if name not in toolkits:
        raise LoadExchangeAttributeError("toolkit", name, toolkits.keys())
    return toolkits[name]
