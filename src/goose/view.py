from attrs import define
from exchange import Exchange


@define
class ExchangeView:
    """A read-only view of the underlying Exchange


    Attributes:
        processor: A copy of the exchange configured for high capabilities
        accelerator: A copy of the exchange configured for high speed

    """

    _processor: str
    _accelerator: str
    _exchange: Exchange

    @property
    def processor(self) -> Exchange:
        return self._exchange.replace(model=self._processor)

    @property
    def accelerator(self) -> Exchange:
        return self._exchange.replace(model=self._accelerator)
