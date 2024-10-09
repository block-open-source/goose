from functools import cache
from typing import Type

from exchange.invalid_choice_error import InvalidChoiceError
from exchange.moderators.base import Moderator
from exchange.utils import load_plugins
from exchange.moderators.passive import PassiveModerator  # noqa
from exchange.moderators.truncate import ContextTruncate  # noqa
from exchange.moderators.summarizer import ContextSummarizer  # noqa


@cache
def get_moderator(name: str) -> Type[Moderator]:
    moderators = load_plugins(group="exchange.moderator")
    if name not in moderators:
        raise InvalidChoiceError("moderator", name, moderators.keys())
    return moderators[name]
