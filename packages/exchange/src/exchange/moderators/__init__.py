from functools import cache
from typing import Type

from exchange.load_exchange_attribute_error import LoadExchangeAttributeError
from exchange.moderators.base import Moderator
from exchange.utils import load_plugins
from exchange.moderators.passive import PassiveModerator  # noqa
from exchange.moderators.truncate import ContextTruncate  # noqa
from exchange.moderators.summarizer import ContextSummarizer  # noqa


@cache
def get_moderator(name: str) -> Type[Moderator]:
    moderators = load_plugins(group="exchange.moderator")
    if name not in moderators:
        raise LoadExchangeAttributeError("moderator", name, moderators.keys())
    return moderators[name]
