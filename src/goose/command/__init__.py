from functools import cache
from typing import Dict

from goose.command.base import Command
from goose.utils import load_plugins


@cache
def get_command(name: str) -> type[Command]:
    return load_plugins(group="goose.command")[name]


@cache
def get_commands() -> Dict[str, type[Command]]:
    return load_plugins(group="goose.command")
