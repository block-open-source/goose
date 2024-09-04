from functools import cache
from typing import Dict

from goose.pluginbase.command import Command
from goose.pluginbase.utils import load_plugins


@cache
def get_command(name: str) -> type[Command]:
    return load_plugins(group="goose.command")[name]


@cache
def get_commands() -> Dict[str, type[Command]]:
    return load_plugins(group="goose.command")
