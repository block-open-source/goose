import json
from pathlib import Path
from typing import Dict, Iterator, List

from exchange import Message

from goose.cli.config import SESSION_FILE_SUFFIX


def write_to_file(file_path: Path, messages: List[Message]) -> None:
    with open(file_path, "w") as f:
        for m in messages:
            json.dump(m.to_dict(), f)
            f.write("\n")


def read_from_file(file_path: Path) -> List[Message]:
    try:
        with open(file_path, "r") as f:
            messages = [json.loads(m) for m in list(f) if m.strip()]
    except json.JSONDecodeError as e:
        raise RuntimeError(f"Failed to load session due to JSON decode Error: {e}")

    return [Message(**m) for m in messages]


def list_sorted_session_files(session_files_directory: Path) -> Dict[str, Path]:
    logs = list_session_files(session_files_directory)
    return {log.stem: log for log in sorted(logs, key=lambda x: x.stat().st_mtime, reverse=True)}


def list_session_files(session_files_directory: Path) -> Iterator[Path]:
    return session_files_directory.glob(f"*{SESSION_FILE_SUFFIX}")


def session_file_exists(session_files_directory: Path) -> bool:
    if not session_files_directory.exists():
        return False
    return any(list_session_files(session_files_directory))
