from exchange import Message  # type: ignore
from goose.toolkit.base import tool  # type: ignore
import re
from goose.toolkit.base import Toolkit


class Jira(Toolkit):
    """Provides an additional prompt on how to interact with Jira"""

    def system(self) -> str:
        """Retrieve detailed configuration and procedural guidelines for Jira operations"""
        template_content = Message.load("prompts/jira.jinja").text
        return template_content

    @tool
    def is_jira_issue(self, issue_key: str) -> str:
        """
        Checks if a given string is a valid JIRA issue key.
        Use this if it looks like the user is asking about a JIRA issue.

        Args:
            issue_key (str): The potential Jira issue key to be validated.

        """
        pattern = r"[A-Z]+-\d+"
        return bool(re.match(pattern, issue_key))
