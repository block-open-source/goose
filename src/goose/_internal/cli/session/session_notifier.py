from goose.notifier import Notifier
from rich.status import Status
from rich.console import RenderableType


class SessionNotifier(Notifier):
    def __init__(self, status_indicator: Status) -> None:
        self.status_indicator = status_indicator

    def log(self, content: RenderableType) -> None:
        print(content)

    def status(self, status: str) -> None:
        self.status_indicator.update(status)
