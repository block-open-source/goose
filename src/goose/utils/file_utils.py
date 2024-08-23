import glob
import os
from collections import Counter
from pathlib import Path
from typing import Dict, List, Optional


def create_extensions_list(project_root: str, max_n: int) -> list:
    """Get the top N file extensions in the current project
    Args:
        project_root (str): Root of the project to analyze
        max_n (int): The number of file extensions to return
    Returns:
        extensions (List[str]): A list of the top N file extensions
    """
    if max_n == 0:
        raise (ValueError("Number of file extensions must be greater than 0"))

    files = create_file_list(project_root, [])

    counter = Counter()

    for file in files:
        file_path = Path(file)
        if file_path.suffix:  # omit ''
            counter[file_path.suffix] += 1

    top_n = counter.most_common(max_n)
    extensions = [ext for ext, _ in top_n]

    return extensions


def create_language_weighting(files_in_directory: List[str]) -> Dict[str, float]:
    """Calculate language weighting by file size to match GitHub's methodology.

    Args:
        files_in_directory (List[str]): Paths to files in the project directory

    Returns:
        Dict[str, float]: A dictionary with languages as keys and their percentage of the total codebase as values
    """

    # Initialize counters for sizes
    size_by_language = Counter()

    # Calculate size for files by language
    for file_path in files_in_directory:
        path = Path(file_path)
        if path.suffix:
            size_by_language[path.suffix] += os.path.getsize(file_path)

    # Calculate total size and language percentages
    total_size = sum(size_by_language.values())
    language_percentages = {
        lang: (size / total_size * 100) if total_size else 0 for lang, size in size_by_language.items()
    }

    return dict(sorted(language_percentages.items(), key=lambda item: item[1], reverse=True))


def list_files_with_extension(dir_path: str, extension: Optional[str] = "") -> List[str]:
    """List all files in a directory with a given extension. Set extension to '' to return all files.

    Args:
        dir_path (str): The path to the directory
        extension (Optional[str]): extension to lookup. Defaults to '' which will return all files.

    Returns:
        files (List[str]): List of file paths
    """
    # add a leading '.' to extension if needed
    if extension and not extension.startswith("."):
        extension = f".{extension}"

    files = glob.glob(f"{dir_path}/**/*{extension}", recursive=True)
    return files


def create_file_list(dir_path: str, extensions: List[str]) -> List[str]:
    """Creates a list of files with certain extensions

    Args:
        dir_path (str): Directory to list files of. Will include files recursively in sub-directories.
        extensions (List[str]): List of file extensions to select for. If empty list, return all files

    Returns:
        final_file_list (List[str]): List of file paths with specified extensions.
    """
    # if extensions is empty list, return all files
    if not extensions:
        return glob.glob(f"{dir_path}/**/*", recursive=True)

    # prune out files that do not end with any of the extensions in extensions
    final_file_list = []
    for ext in extensions:
        if ext and not ext.startswith("."):
            ext = f".{ext}"

        files = glob.glob(f"{dir_path}/**/*{ext}", recursive=True)
        final_file_list += files

    return final_file_list
