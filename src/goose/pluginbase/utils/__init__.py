import random
import string
from importlib.metadata import entry_points
from typing import Any, Callable, Dict, List, Type, TypeVar

T = TypeVar("T")


def load_plugins(group: str) -> dict:
    """
    Load plugins based on a specified entry point group.

    This function iterates through all entry points registered under a specified group

    Args:
        group (str): The entry point group to load plugins from. This should match the group specified
                     in the package setup where plugins are defined.

    Returns:
        dict: A dictionary where each key is the entry point name, and the value is the loaded plugin object.

    Raises:
        Exception: Propagates exceptions raised by entry point loading, which might occur if a plugin
                   is not found or if there are issues with the plugin's code.
    """
    plugins = {}
    # Access all entry points for the specified group and load each.
    for entrypoint in entry_points(group=group):
        plugin = entrypoint.load()  # Load the plugin.
        plugins[entrypoint.name] = plugin  # Store the loaded plugin in the dictionary.
    return plugins


def ensure(cls: Type[T]) -> Callable[[Any], T]:
    """Convert dictionary to a class instance"""

    def converter(val: Any) -> T:  # noqa: ANN401
        if isinstance(val, cls):
            return val
        elif isinstance(val, dict):
            return cls(**val)
        elif isinstance(val, list):
            return cls(*val)
        else:
            return cls(val)

    return converter


def ensure_list(cls: Type[T]) -> Callable[[List[Dict[str, Any]]], Type[T]]:
    """Convert a list of dictionaries to class instances"""

    def converter(val: List[Dict[str, Any]]) -> List[T]:
        output = []
        for entry in val:
            output.append(ensure(cls)(entry))
        return output

    return converter


def droid() -> str:
    return "".join(
        [
            random.choice(string.ascii_lowercase),
            random.choice(string.digits),
            random.choice(string.ascii_lowercase),
            random.choice(string.digits),
        ]
    )
