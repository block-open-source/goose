import glob
from typing import List


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
