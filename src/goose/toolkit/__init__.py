from functools import cache
from exchange.invalid_choice_error import InvalidChoiceError
from goose.toolkit.base import Toolkit
from goose.utils import load_plugins


@cache
def get_toolkit(name: str) -> type[Toolkit]:
    toolkits = load_plugins(group="goose.toolkit")
    if name not in toolkits:
        raise InvalidChoiceError("toolkit", name, toolkits.keys())
    return toolkits[name]
