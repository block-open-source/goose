from abc import ABC
from typing import List, Optional

from prompt_toolkit.completion import Completion


class Command(ABC):
    """A command that can be executed by the CLI."""

    def get_completions(self, query: str) -> List[Completion]:
        """Get completions for the command."""
        return []

    def execute(self, query: str) -> Optional[str]:
        """Execute's the command and replaces it with the output."""
        return ""
