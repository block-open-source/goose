from abc import ABC, abstractmethod

from rich.console import RenderableType


class Notifier(ABC):
    """The interface for a notifier

    This is expected to be implemented concretely by the each UX
    """

    @abstractmethod
    def log(self, content: RenderableType) -> None:
        """Append content to the main display

        Args:
            content (str): The content to render
        """
        pass

    @abstractmethod
    def status(self, status: str) -> None:
        """Log a status to ephemeral display

        Args:
            status (str): The status to display
        """
        pass
