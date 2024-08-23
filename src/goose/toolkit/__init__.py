from functools import cache

from goose.toolkit.base import Toolkit
from goose.utils import load_plugins


@cache
def get_toolkit(name: str) -> type[Toolkit]:
    return load_plugins(group="goose.toolkit")[name]
