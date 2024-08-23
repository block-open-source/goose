import os
from typing import List, Optional

from goose.toolkit import Toolkit
from goose.toolkit.base import tool
from goose.toolkit.summarization.utils import summarize_directory


class SummarizeProject(Toolkit):
    @tool
    def get_project_summary(
        self,
        project_dir_path: Optional[str] = os.getcwd(),
        extensions: Optional[List[str]] = None,
        summary_instructions_prompt: Optional[str] = None,
    ) -> dict:
        """Generates or retrieves a project summary based on specified file extensions.

        Args:
            project_dir_path (Optional[Path]): Path to the project directory. Defaults to the current working directory
                if None
            extensions (Optional[List[str]]): Specific file extensions to summarize.
            summary_instructions_prompt (Optional[str]): Instructions to give to the LLM about how to summarize each file. E.g.
                "Summarize the file in two sentences.". The default instruction is "Please summarize this file."

        Returns:
            summary (dict): Project summary.
        """  # noqa: E501

        summary = summarize_directory(
            project_dir_path,
            exchange=self.exchange_view.accelerator,
            extensions=extensions,
            summary_instructions_prompt=summary_instructions_prompt,
        )

        return summary
