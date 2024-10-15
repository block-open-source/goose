import functools
import math
from typing import Callable, List, Optional, Tuple, Type

from exchange.message import Message
from rich.markdown import Markdown
from goose.language_server.client import LanguageServerClient
from goose.language_server.config import LanguageServerConfig
from goose.language_server.language_server_types import Location
from goose.language_server.logger import LanguageServerLogger
from rich.prompt import Confirm
from rich import print
from goose.notifier import Notifier
from goose.toolkit.base import Requirements, Toolkit, tool
from goose.utils import load_plugins
from goose.utils.file_utils import get_code_snippet


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

        for name, language_server_cls in load_plugins("goose.language_server").items():
            try:
                ls = language_server_cls.from_env(config=language_server_config, logger=language_server_logger)
                is_enabled = True
                # TODO: undo
                # is_enabled = Confirm.ask(
                #     f"Would you like to enable the [blue bold]{name}[/] language server?", default=True
                # )
                if is_enabled:
                    self.language_server_client.register_language_server(ls)
            except Exception:
                print(f"[red]Failed to initialize the {name} language server[/]")

        if not self.language_server_client.language_servers:
            self.language_server_client = None
            return

        developer_toolkit_instance = requires.get("developer")

        def method_changes_file(func: Callable) -> Callable:
            @functools.wraps(func)
            def wrap_method(*args: list, **kwargs: dict) -> Callable:
                with self.language_server_client.open_file(kwargs.get("file_path")):
                    result = func(*args, **kwargs)
                    self.language_server_client
                return result

            wrap_method._is_method = True
            return wrap_method

        for method in ["write_file", "patch_file"]:
            decorated_method = method_changes_file(getattr(developer_toolkit_instance, method))
            setattr(
                developer_toolkit_instance,
                method,
                decorated_method,
            )

    def system(self) -> str:
        if not self.language_server_client:
            return ""
        languages = list(self.language_server_client.language_servers.keys())
        return Message.load("prompts/language_server.jinja", dict(languages=", ".join(languages))).text

    def get_readable_lsp_results(self, results: List[Location], current_page: int, total_pages: int) -> List[str]:
        human_readable_results = []
        for result in results:
            file_path = result["absolutePath"]
            start_line = result["range"]["start"]["line"]
            end_line = result["range"]["end"]["line"] + 1  # because end is exclusive
            human_readable_results.append(
                get_code_snippet(
                    file_path=file_path,
                    start_line=start_line,
                    end_line=end_line,
                )
            )

        all_results = "\n".join(human_readable_results)
        self.notifier.log(Markdown(f"## Results (Page {current_page + 1} of {total_pages})\n{all_results}"))
        return human_readable_results

    @tool
    def request_definition(
        self, file_path: str, line: int, column: int, page_number: int = 0, page_size: int = 50
    ) -> dict:
        """
        Requests the definition of a symbol at a given position in a file.

        Args:
            file_path (str): The path to the file.
            line (int): The line number of the symbol.
            column (int): The column number of the symbol.
            page_number (int, optional): The requested page number of the results.
            page_size (int, optional): The number of results per page
        """
        if not self.language_server_client:
            NotImplementedError("No language server is available.")
        results = self.language_server_client.request_definition(file_path, line, column)

        # TODO: paginate results

        if not results:
            return "No definition found."

        total_pages = math.ceil(len(results) / page_size)
        return dict(
            results=self.get_readable_lsp_results(
                results,  # replace with paginated results
                current_page=page_number + 1,  # because page_number is 0-indexed
                total_pages=total_pages,
            ),
            current_page_number=page_number + 1,  # because page_number is 0-indexed
            total_pages=total_pages,
        )

    @tool
    def request_references(
        self, file_path: str, line: int, column: int, page_number: int = 0, page_size: int = 50
    ) -> dict:
        """
        Requests the references of a symbol at a given position in a file.

        Args:
            file_path (str): The path to the file.
            line (int): The line number of the symbol.
            column (int): The column number of the symbol.
            page_number (int, optional): The requested page number of the results.
            page_size (int, optional): The number of results per page
        """
        if not self.language_server_client:
            NotImplementedError("No language server is available.")
        results = self.language_server_client.request_references(file_path, line, column)

        # TODO: paginate results

        if not results:
            return "No definition found."

        total_pages = math.ceil(len(results) / page_size)
        return dict(
            results=self.get_readable_lsp_results(
                results,  # replace with paginated results
                current_page=page_number,
                total_pages=total_pages,
            ),
            current_page_number=page_number,
            total_pages=total_pages,
        )
