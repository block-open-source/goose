import ast
import concurrent.futures
import os
from collections import deque
from typing import Dict, List, Tuple

from exchange import Exchange

from goose.utils.ask import ask_an_ai


def get_directory_size(directory: str) -> int:
    total_size = 0
    for dirpath, _, filenames in os.walk(directory):
        for f in filenames:
            fp = os.path.join(dirpath, f)
            # Skip if it is a symbolic link
            if not os.path.islink(fp):
                total_size += os.path.getsize(fp)
    return total_size


def get_repo_size(repo_path: str) -> int:
    """Returns repo size in MB"""
    git_dir = os.path.join(repo_path, ".git")
    return get_directory_size(git_dir) / (1024**2)


def get_files_and_directories(root_dir: str) -> Dict[str, list]:
    """Gets file names and directory names. Checks that goose has correctly typed the file and directory names and that
    the files actually exist (to avoid downstream file read errors).

    Args:
        root_dir (str): Path to the directory to examine for files and sub-directories

    Returns:
        dict: A list of files and directories in the form {'files': [], 'directories: []}. Paths
            are all relative (i.e. ['src'] not ['goose/src'])
    """
    files = []
    dirs = []

    # check dir exists
    try:
        os.listdir(root_dir)
    except FileNotFoundError:
        # FIXME: fuzzy match might work here to recover directories 'lost' to goose mistyping
        # hallucination: Goose mistyped the path (e.g. `metrichandler` vs `metricshandler`)
        return {"files": files, "directories": dirs}

    for entry in os.listdir(root_dir):
        if entry.startswith(".") or entry.startswith("~"):
            continue  # Skip hidden files and directories

        full_path = os.path.join(root_dir, entry)
        if os.path.isdir(full_path):
            dirs.append(entry)
        elif os.path.isfile(full_path):
            files.append(entry)

    return {"files": files, "directories": dirs}


def goose_picks_files(root: str, exchange: Exchange, max_workers: int = 4) -> List[str]:
    """Lets goose pick files in a BFS manner"""
    queue = deque([root])

    all_files = []

    with concurrent.futures.ThreadPoolExecutor(max_workers=max_workers) as executor:
        while queue:
            current_batch = [queue.popleft() for _ in range(min(max_workers, len(queue)))]
            futures = {executor.submit(process_directory, dir, exchange): dir for dir in current_batch}

            for future in concurrent.futures.as_completed(futures):
                files, next_dirs = future.result()
                all_files.extend(files)
                queue.extend(next_dirs)

    return all_files


def process_directory(current_dir: str, exchange: Exchange) -> Tuple[List[str], List[str]]:
    """Allows goose to pick files and subdirectories contained in a given directory (current_dir). Get the list of file
    and directory names in the current folder, then ask Goose to pick which ones to keep.

    """
    files_and_dirs = get_files_and_directories(current_dir)
    ai_response = ask_an_ai(str(files_and_dirs), exchange)

    # FIXME: goose response validation
    try:
        as_dict = ast.literal_eval(ai_response.text)
    except Exception:
        # can happen if goose returns anything but {result: dict} (e.g. ```json\n {results: dict} \n```)
        return [], []
    if not isinstance(as_dict, dict):
        # can happen if goose returns something like `{'files': ['x.py'] 'directories': ['dir1']}` (missing comma)
        return [], []

    files = [f"{current_dir}/{file}" for file in as_dict.get("files", [])]
    next_dirs = [f"{current_dir}/{next_dir}" for next_dir in as_dict.get("directories", [])]

    return files, next_dirs
