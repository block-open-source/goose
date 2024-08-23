from exchange import Message

from goose.toolkit.base import Toolkit


class Github(Toolkit):
    """Provides an additional prompt on how to interact with Github"""

    def system(self) -> str:
        """Retrieve detailed configuration and procedural guidelines for GitHub operations"""
        return Message.load("prompts/github.jinja").text
