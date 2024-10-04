from rich.status import Status
from rich.live import Live
from rich.console import RenderableType
from rich import print

from goose.notifier import Notifier


class SessionNotifier(Notifier):
    def __init__(self, status_indicator: Status) -> None:
        self.status_indicator = status_indicator
        self.live = Live(self.status_indicator, refresh_per_second=8, transient=True)

    def log(self, content: RenderableType) -> None:
        print(content)

    def status(self, status: str) -> None:
        self.status_indicator.update(status)

    def start(self) -> None:
        self.live.start()

    def stop(self) -> None:
        self.live.stop()
