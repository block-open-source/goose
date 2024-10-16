from abc import ABC, abstractmethod


class Moderator(ABC):
    @abstractmethod
    def rewrite(self, exchange: type["exchange.exchange.Exchange"]) -> None:  # noqa: F821
        pass
