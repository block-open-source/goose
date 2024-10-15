from functools import cache

from goose.command.base import Command
from goose.utils import load_plugins


@cache
def get_command(name: str) -> type[Command]:
    return load_plugins(group="goose.command")[name]


@cache
def get_commands() -> dict[str, type[Command]]:
    return load_plugins(group="goose.command")
