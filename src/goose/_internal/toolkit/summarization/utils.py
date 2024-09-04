import json
import subprocess
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path
from typing import Dict, List, Optional, Tuple

from exchange import Exchange
from exchange.providers.utils import InitialMessageTooLargeError

from goose.utils.ask import ask_an_ai
from goose.utils.file_utils import create_file_list

SUMMARIES_FOLDER = ".goose/summaries"
CLONED_REPOS_FOLDER = ".goose/cloned_repos"


# TODO: move git stuff
def run_git_command(command: List[str]) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(["git"] + command, capture_output=True, text=True, check=False)

    if result.returncode != 0:
        raise Exception(f"Git command failed with message: {result.stderr.strip()}")

    return result


def clone_repo(repo_url: str, target_directory: str) -> None:
    run_git_command(["clone", repo_url, target_directory])


def load_summary_file_if_exists(project_name: str) -> Optional[Dict]:
    """Checks if a summary file exists at '.goose/summaries/projectname-summary.json. Returns contents of the file if
    it exists, otherwise returns None

    Args:
        project_name (str): name of the project or repo

    Returns:
        Optional[Dict]: File contents, else None
    """
    summary_file_path = f"{SUMMARIES_FOLDER}/{project_name}-summary.json"
    if Path(summary_file_path).exists():
        with open(summary_file_path, "r") as f:
            return json.load(f)


def summarize_file(filepath: str, exchange: Exchange, prompt: Optional[str] = None) -> Tuple[str, str]:
    """Summarizes a single file

    Args:
        filepath (str): Path to the file to summarize.
        exchange (Exchange): Exchange object to use for summarization.
        prompt (Optional[str]): Defaults to "Please summarize this file."
    """
    try:
        with open(filepath, "r") as f:
            file_text = f.read()
    except Exception as e:
        return filepath, f"Error reading file {filepath}: {str(e)}"

    if not file_text:
        return filepath, "Empty file"

    try:
        reply = ask_an_ai(
            input=file_text, exchange=exchange, prompt=prompt if prompt else "Please summarize this file."
        )
    except InitialMessageTooLargeError:
        return filepath, "File too large"

    return filepath, reply.text


def summarize_repo(
    repo_url: str,
    exchange: Exchange,
    extensions: List[str],
    summary_instructions_prompt: Optional[str] = None,
) -> Dict[str, str]:
    """Clones (if needed) and summarizes a repo

    Args:
        repo_url (str): Repository url
        exchange (Exchange): Exchange for summarizing the repo.
        extensions (List[str]): List of file-types to summarize.
        summary_instructions_prompt (Optional[str]): Optional parameter to customize summarization results. Defaults to
            "Please summarize this file"
    """
    # set up the paths for the repository and the summary file
    repo_name = repo_url.split("/")[-1]
    repo_dir = f"{CLONED_REPOS_FOLDER}/{repo_name}"  # e.g. '.goose/cloned_repos/<project-name>'

    if Path(repo_dir).exists():
        # TODO: re-add ability to log
        return summarize_directory(
            directory=repo_dir,
            exchange=exchange,
            extensions=extensions,
            summary_instructions_prompt=summary_instructions_prompt,
        )

    clone_repo(repo_url, target_directory=repo_dir)

    return summarize_directory(
        directory=repo_dir,
        exchange=exchange,
        extensions=extensions,
        summary_instructions_prompt=summary_instructions_prompt,
    )


def summarize_directory(
    directory: str, exchange: Exchange, extensions: List[str], summary_instructions_prompt: Optional[str] = None
) -> Dict[str, str]:
    """Summarize files in a given directory based on extensions. Will also recursively find files in subdirectories and
    summarize them.

    Args:
        directory (str): path to the top-level directory to summarize
        exchange (Exchange): Exchange to use to summarize
        extensions (List[str]): List of file-type extensions to summarize (and ignore all other extensions).
        summary_instructions_prompt (Optional[str]): Optional instructions to give to the exchange regarding summarization.

    Returns:
        file_summaries (dict): Keys are file names and values are summaries.

    """  # noqa: E501

    # TODO: make sure that '.goose/summaries' is
    # in the root of the current not relative to current dir or in cloned repo root
    project_name = directory.split("/")[-1]
    summary_file = load_summary_file_if_exists(project_name)
    if summary_file:
        return summary_file

    summary_file_path = f"{SUMMARIES_FOLDER}/{project_name}-summary.json"

    # create the .goose/summaries folder if not already created
    Path(SUMMARIES_FOLDER).mkdir(exist_ok=True, parents=True)

    # select a subset of files to summarize based on file extension
    files_to_summarize = create_file_list(directory, extensions=extensions)

    file_summaries = summarize_files_concurrent(
        exchange=exchange,
        file_list=files_to_summarize,
        project_name=project_name,
        summary_instructions_prompt=summary_instructions_prompt,
    )

    summary_file_contents = {"extensions": extensions, "summaries": file_summaries}

    # Write the summaries into a json
    with open(summary_file_path, "w") as f:
        json.dump(summary_file_contents, f, indent=2)

    return file_summaries


def summarize_files_concurrent(
    exchange: Exchange, file_list: List[str], project_name: str, summary_instructions_prompt: Optional[str] = None
) -> Dict[str, str]:
    """Takes in a list of files and summarizes them. Exchange does not keep history of the summarized files.

    Args:
        exchange (Exchange): Underlying exchange
        file_list (List[str]): List of paths to files to summarize
        project_name (str): Used to save the summary of the files to .goose/summaries/<project_name>-summary.json
        summary_instructions_prompt (Optional[str]): Summary instructions for the LLM. Defaults to "Please summarize
            this file."

    Returns:
        file_summaries (Dict[str, str]): Keys are file paths and values are the summaries returned by the Exchange
    """
    summary_file = load_summary_file_if_exists(project_name)
    if summary_file:
        return summary_file

    file_summaries = {}
    # compile the individual file summaries into a single summary dict
    # TODO: add progress bar as this step can take quite some time and it's nice to see something is happening
    with ThreadPoolExecutor() as executor:
        future_to_file = {
            executor.submit(summarize_file, file, exchange, summary_instructions_prompt): file for file in file_list
        }

        for future in as_completed(future_to_file):
            file_name, file_summary = future.result()
            file_summaries[file_name] = file_summary

    # create summaries folder if it doesn't exist
    Path(SUMMARIES_FOLDER).mkdir(exist_ok=True, parents=True)
    summary_file_path = f"{SUMMARIES_FOLDER}/{project_name}-summary.json"

    # Write the summaries into a json
    with open(summary_file_path, "w") as f:
        json.dump(file_summaries, f, indent=2)

    return file_summaries
