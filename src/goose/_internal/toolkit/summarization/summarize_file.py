from typing import Optional

from goose.pluginbase.toolkit import Toolkit, tool
from goose.pluginbase.utils.summarization import summarize_file


class SummarizeFile(Toolkit):
    @tool
    def summarize_file(self, filepath: str, prompt: Optional[str] = None) -> str:
        """
        Tool to summarize a specific file

        Args:
            filepath (str): Path to the file to summarize
            prompt (str): Optional prompt giving the model instructions on how to summarize the file.
                Under the hood this defaults to "Please summarize this file"

        Returns:
            summary (Optional[str]): Summary of the file contents

        """

        exchange = self.exchange_view.accelerator

        _, summary = summarize_file(filepath=filepath, exchange=exchange, prompt=prompt)

        return summary
