from functools import cache
from typing import Type

from exchange.moderators.base import Moderator
from exchange.utils import load_plugins
from exchange.moderators.passive import PassiveModerator  # noqa
from exchange.moderators.truncate import ContextTruncate  # noqa
from exchange.moderators.summarizer import ContextSummarizer  # noqa


@cache
def get_moderator(name: str) -> Type[Moderator]:
    return load_plugins(group="exchange.moderator")[name]
