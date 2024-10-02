from typing import Type
from exchange.moderators.base import Moderator


class PassiveModerator(Moderator):
    def rewrite(self, _: Type["exchange.exchange.Exchange"]) -> None:  # noqa: F821
        pass
