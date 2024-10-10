import json
import os
import tempfile
from pathlib import Path
from typing import Dict, Iterator, List

from exchange import Message

from goose.cli.config import SESSION_FILE_SUFFIX


def is_existing_session(path: Path) -> bool:
    return path.is_file() and path.stat().st_size > 0


def is_empty_session(path: Path) -> bool:
    return path.is_file() and path.stat().st_size == 0


def write_to_file(file_path: Path, messages: List[Message]) -> None:
    with open(file_path, "w") as f:
        _write_messages_to_file(f, messages)


def read_or_create_file(file_path: Path) -> List[Message]:
    if file_path.exists():
        return read_from_file(file_path)
    with open(file_path, "w"):
        pass
    return []


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


def save_latest_session(file_path: Path, messages: List[Message]) -> None:
    with tempfile.NamedTemporaryFile("w", delete=False) as temp_file:
        _write_messages_to_file(temp_file, messages)
        temp_file_path = temp_file.name

    os.replace(temp_file_path, file_path)


def _write_messages_to_file(file: any, messages: List[Message]) -> None:
    for m in messages:
        json.dump(m.to_dict(), file)
        file.write("\n")
