from typing import List, Optional

from goose.toolkit import Toolkit
from goose.toolkit.base import tool
from goose.toolkit.summarization.utils import summarize_repo


class SummarizeRepo(Toolkit):
    @tool
    def summarize_repo(
        self,
        repo_url: str,
        specified_extensions: Optional[List[str]] = None,
        summary_instructions_prompt: Optional[str] = None,
    ) -> dict:
        """
        Retrieves a summary of a repository. Clones the repository if not already cloned and summarizes based on the
        specified file extensions. If no extensions are specified, it summarizes the top `max_extensions` extensions.

        Args:
            repo_url (str): The URL of the repository to summarize.
            specified_extensions (Optional[List[str]]): List of file extensions to summarize, e.g., ["tf", "md"]. If
                this list is empty, then all files in the repo are summarized
            summary_instructions_prompt (Optional[str]): Instructions to give to the LLM about how to summarize each file. E.g.
                "Summarize the file in two sentences.". The default instruction is "Please summarize this file."

        Returns:
            summary (dict): A summary of the repository where keys are the file extensions and values are their
                summaries.
        """  # noqa: E501

        return summarize_repo(
            repo_url=repo_url,
            exchange=self.exchange_view.accelerator,
            extensions=specified_extensions,
            summary_instructions_prompt=summary_instructions_prompt,
        )
