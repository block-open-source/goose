import json
from tiktoken import get_encoding
from exchange import Message
import subprocess
import os
import atexit
import platform
from pathlib import Path
from typing import Dict, Iterable, List, Set

from attrs import define, field
from exchange.content import ToolUse
from goose.toolkit.utils import get_language


@define
class File:
    path: str
    content: str
    language: str

    @property
    def context(self) -> str:
        return f"""\
{self.path}
```{self.language}
{self.content}
```"""


@define
class OperatingSystem:
    """
    OperatingSystem class that can manage background processes created using subprocess.Popen.
    """

    cwd: str = os.getcwd()
    platform: str = platform.system()
    env: Dict[str, str] = os.environ.copy()
    _active_files: Set[str] = field(init=False, factory=set)
    _processes: Dict[int, subprocess.Popen] = field(init=False, factory=dict)

    def __attrs_post_init__(self) -> None:
        atexit.register(self._cleanup_processes)

    def _cleanup_processes(self) -> None:
        """Terminate all processes in the process queue."""
        for process_id in list(self._processes.keys()):
            self.cancel_process(process_id)

    def add_process(self, process: subprocess.Popen) -> int:
        """Add a new background process and return its assigned ID."""
        process_id = process.pid
        os.set_blocking(process.stdout.fileno(), False)
        self._processes[process_id] = process
        return process_id

    def get_processes(self) -> Dict[int, str]:
        """List all background processes with their IDs and commands."""
        return {pid: str(proc.args) for pid, proc in self._processes.items()}

    def view_process_output(self, process_id: int) -> str:
        """View the output of a running background process."""
        if not (process := self._processes.get(process_id)):
            raise ValueError(f"No process found with ID: {process_id}")

        output = []
        while line := process.stdout.readline():
            output.append(line)

        return "".join(output)

    def cancel_process(self, process_id: int) -> bool:
        """Cancel the background process with the specified ID."""
        process = self._processes.pop(process_id, None)
        if process:
            process.terminate()
            return True
        return False

    def to_relative(self, path: str) -> str:
        """convert string path to be relative to cwd, assuming it starts absolute"""
        return os.path.relpath(path, start=self.cwd)

    def to_patho(self, path: str) -> Path:
        """convert string to pathlib.Path object

        This attempts to resolve the path against the cwd
        """
        patho = Path(path)
        if patho.is_absolute():
            return patho

        return (self.cwd / patho).resolve()

    def remember_file(self, path: str) -> None:
        """Place a file into the active files"""
        path = str(self.to_patho(path))

        # Do a size check on the file to ensure we don't overload the LLM context
        with open(path, "r") as f:
            content = f.read()

        max_output_chars = 2**20
        max_output_tokens = 16000
        encoder = get_encoding("cl100k_base")

        if len(content) > max_output_chars or len(encoder.encode(content)) > max_output_tokens:
            raise ValueError(f"The file at {path} is too large to read directly!")

        self._active_files.add(path)

    def forget_file(self, path: str) -> None:
        """Forget an existing active file"""
        self._active_files.discard(str(self.to_patho(path)))

    def info(self) -> str:
        """Summarize the current operating system"""
        return json.dumps(
            dict(
                os=self.platform,
                cwd=str(self.to_patho(self.cwd)),
                shell=os.environ.get("SHELL", "unknown"),
            ),
            indent=4,
        )

    def is_active(self, path: str) -> bool:
        resolved = self.to_patho(path)
        return str(resolved) in self._active_files

    @property
    def active_files(self) -> Iterable["File"]:
        """Yield a File instance for each path in active files, with paths relative to cwd."""
        self._active_files = set(f for f in self._active_files if Path(f).exists())

        for path in self._active_files:
            yield File(path=self.to_relative(path), content=Path(path).read_text(), language=get_language(path))

    def restore(self, messages: List[Message]) -> None:
        """Restore the file content space from a previous sessions"""
        for message in messages:
            for content in message.content:
                if isinstance(content, ToolUse) and content.name == "read_file":
                    self.remember_file(content.parameters["path"])


system = OperatingSystem()
