import functools
import types
from typing import Any, Callable, List, Optional, Tuple, Type, TypeVar
from goose.language_server.client import LanguageServerClient
from goose.language_server.config import LanguageServerConfig
from goose.language_server.logger import LanguageServerLogger
from goose.notifier import Notifier
from goose.toolkit.base import Requirements, Toolkit, tool
from goose.utils import load_plugins


def method_changes_file(func: Callable) -> Callable:
    @functools.wraps(func)
    def wrap_method(*args: list, **kwargs: dict) -> Callable:
        print(f"Before calling {func.__name__}")
        result = func(*args, **kwargs)
        print(f"After calling {func.__name__}")
        return result

    wrap_method._is_method = True
    return wrap_method


class LanguageServerCoordinator(Toolkit):
    _instance: Optional["LanguageServerCoordinator"] = None

    def __new__(cls, *args: tuple, **kwargs: dict) -> "LanguageServerCoordinator":
        if cls._instance is None:
            cls._instance = super(LanguageServerCoordinator, cls).__new__(cls)
        return cls._instance

    @classmethod
    def get_instance(cls: Type["LanguageServerCoordinator"]) -> "LanguageServerCoordinator":
        """Returns the singleton instance of the LanguageServerCoordinator."""
        if not cls._instance:
            raise ValueError("LanguageServerCoordinator has not been initialized.")
        return cls._instance

    def __init__(self, notifier: Notifier, requires: Optional[Requirements] = None) -> None:
        super().__init__(notifier=notifier, requires=requires)

        language_server_logger = LanguageServerLogger()
        language_server_config = LanguageServerConfig(trace_lsp_communication=False)
        self.language_server_client = LanguageServerClient()

        for language_server_cls in load_plugins("goose.language_server").values():
            ls = language_server_cls.from_env(config=language_server_config, logger=language_server_logger)
            self.language_server_client.register_language_server(ls)

        developer_toolkit_instance = requires.get("developer")
        for method in ["write_file", "patch_file"]:
            decorated_method = method_changes_file(getattr(developer_toolkit_instance, method))
            setattr(
                developer_toolkit_instance,
                method,
                decorated_method,
            )

    @tool
    def request_definition(self, file_path: str, line: int, column: int) -> List[Tuple[str, int]]:
        """
        Requests the definition of a symbol at a given position in a file.

        Args:
            file_path (str): The path to the file.
            line (int): The line number of the symbol.
            column (int): The column number of the symbol.
        """
        results = self.language_server_client.request_definition(file_path, line, column)

        return [dict(path=result.absolute_path, line_num=result.range.start) for result in results]

    @tool
    def request_references(self, file_path: str, line: int, column: int) -> List[Tuple[str, int]]:
        """
        Requests the references of a symbol at a given position in a file.

        Args:
            file_path (str): The path to the file.
            line (int): The line number of the symbol.
            column (int): The column number of the symbol.
        """
        results = self.language_server_client.request_references(file_path, line, column)
        return [dict(path=result.absolute_path, line_num=result.range.start) for result in results]

    @tool
    def request_hover(self, file_path: str, line: int, column: int) -> str | None:
        """
        Requests hover information for a symbol at a given position in a file.

        Args:
            file_path (str): The path to the file.
            line (int): The line number of the symbol.
            column (int): The column number of the symbol.
        """
        result = self.language_server_client.request_hover(file_path, line, column).value
        return result.value if result is not None else None
