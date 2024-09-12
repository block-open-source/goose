import os
from functools import cache
from subprocess import CompletedProcess, run
from typing import Dict, Tuple

from exchange import Message

from goose.notifier import Notifier
from goose.toolkit import Toolkit
from goose.toolkit.base import Requirements, tool
from goose.toolkit.repo_context.utils import get_repo_size, goose_picks_files
from goose.toolkit.summarization.utils import load_summary_file_if_exists, summarize_files_concurrent
from goose.utils.ask import clear_exchange, replace_prompt


class RepoContext(Toolkit):
    """Provides context about the current repository"""

    def __init__(self, notifier: Notifier, requires: Requirements) -> None:
        super().__init__(notifier=notifier, requires=requires)

        self.repo_project_root, self.is_git_repo, self.goose_session_root = self.determine_git_proj()

    def determine_git_proj(self) -> Tuple[str, bool, str]:
        """Determines the root as well as where Goose is currently running

        If the project is not part of a Github repo, the root of the project will be defined as the current working
        directory

        Returns:
            str: path to the root of the project (if part of a local repository) or the CWD if not
            boolean: if Goose is operating within local repository or not
            str: path to where the Goose session is running (the CWD)
        """
        # FIXME: monorepos
        cwd = os.getcwd()
        command = "git rev-parse --show-toplevel"
        result: CompletedProcess = run(command, shell=True, text=True, capture_output=True, check=False)
        if result.returncode == 0:
            project_root = result.stdout.strip()
            return project_root, True, cwd
        else:
            self.notifier.log("Not part of a Git repository. Returning current working directory")
            return cwd, False, cwd

    @property
    @cache
    def repo_size(self) -> float:
        """Returns the size of the repo in MB (if Goose detects its running in a local repository

        This measurement can be used to guess if the local repository is a monorepo

        Returns:
            float: size of project in MB
        """
        # in MB
        if self.is_git_repo:
            return get_repo_size(self.repo_project_root)
        else:
            self.notifier.log("Not a git repo. Returning 0.")
            return 0.0

    @property
    def is_mono_repo(self) -> bool:
        """An boolean indicator of whether the local repository is part of a monorepo

        Returns:
            boolean: True if above 2000 MB; False otherwise
        """
        # java: 6394.367112159729
        # go: 3729.93 MB
        return self.repo_size > 2000

    @tool
    def summarize_current_project(self) -> Dict[str, str]:
        """Summarizes the current project based on repo root (if git repo) or current project_directory (if not)

        Returns:
            summary (Dict[str, str]): Keys are file paths and values are the summaries
        """

        self.notifier.log("Summarizing the most relevant files in the current project. This may take a while...")

        if self.is_mono_repo:
            self.notifier.log("This might be a monorepo. Goose performs better on smaller projects. Using CWD.")
            # TODO: prompt user to specify a subdirectory
            project_directory = self.goose_session_root
        else:
            project_directory = self.repo_project_root

        # before selecting files and summarizing look for summarization file
        project_name = project_directory.split("/")[-1]
        summary = load_summary_file_if_exists(project_name=project_name)
        if summary:
            self.notifier.log("Summary file for project exists already -- loading into the context")
            return summary

        # clear exchange and replace the system prompt with instructions on why and how to select files to summarize
        file_select_exchange = clear_exchange(self.exchange_view.accelerator, clear_tools=True)
        system = Message.load("prompts/repo_context.jinja").text
        file_select_exchange = replace_prompt(exchange=file_select_exchange, prompt=system)
        files = goose_picks_files(root=project_directory, exchange=file_select_exchange)

        summary = summarize_files_concurrent(
            exchange=self.exchange_view.accelerator, file_list=files, project_name=project_directory.split("/")[-1]
        )

        return summary
