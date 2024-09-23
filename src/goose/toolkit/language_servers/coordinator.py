from collections import defaultdict
import logging
import threading
from typing import List, Optional
from goose.language_server.base import LanguageServer, SyncLanguageServer
from goose.language_server.config import Language, MultilspyConfig
from goose.language_server.implementations.jedi import JediServer
from goose.language_server.logger import MultilspyLogger
from goose.notifier import Notifier
import goose.language_server.types as multilspy_types
from goose.toolkit.base import Requirements, Toolkit, tool
from exchange import Message

from goose.utils import load_plugins

class NoSupportedLanguageServerError(Exception):
    pass

class LanguageServerThread(threading.Thread):
    def __init__(self, syncronous_ls: SyncLanguageServer):
        super().__init__()
        self.syncronous_ls = syncronous_ls
        self.request_event = threading.Event()
        self.requests = []

    def run(self):
        with self.syncronous_ls:
            while True:
                self.request_event.wait()  # Wait for a request to be signaled
                if not self.requests:
                    continue
                file_path, line, column, result_container = self.requests.pop(0)
                try:
                    result = self.syncronous_ls.request_definition(file_path, line, column)
                    result_container.append(str(result))
                except Exception as e:
                    logging.error(f"Error requesting definition from {self.syncronous_ls}: {e}")
                self.request_event.clear()

    def request_definition(self, file_path: str, line: int, column: int, result_container: List[str]):
        self.requests.append((file_path, line, column, result_container))
        self.request_event.set()

class LanguageServerCoordinator(Toolkit):
    def __init__(self, notifier: Notifier, _: Optional[Requirements] = None) -> None:
        super().__init__(notifier=notifier, requires=_)
        language_server_logger = MultilspyLogger()
        language_server_config = MultilspyConfig(trace_lsp_communication=False)

        # read in all available language servers
        self.language_servers: dict[str, List[LanguageServerThread]] = defaultdict(list)
        for language_server_cls in load_plugins("goose.language_server").values():
            ls = language_server_cls.from_env(env=language_server_config, logger=language_server_logger)
            self.setup_server(ls)

    def setup_server(self, language_server: LanguageServer) -> None:
        for language in language_server.supported_languages():
            syncronous_ls = SyncLanguageServer(language_server)
            server_thread = LanguageServerThread(syncronous_ls)
            server_thread.start()
            self.language_servers[language].append(server_thread)

    def system(self) -> str:
        return Message.load("prompts/language_server.jinja").text

    def _ensure_valid_request(self, file_path: str) -> None:
        try:
            language = Language.from_file_path(file_path)
        except ValueError:
            raise NoSupportedLanguageServerError(f"No language servers found for {file_path} is not supported by Goose")
        if language not in self.language_servers.keys():
            raise NoSupportedLanguageServerError(f"No language servers were initialized for language {language}")

    @tool
    def request_definition(self, file_path: str, line: int, column: int) -> str:
        """
        Request the definition of a symbol at a given position in a file.

        Args:
            file_path (str): Absolute path to the file.
            line (int): Line number of the symbol.
            column (int): Column number for the symbol on the selected line.

        Returns:
            str: The definition(s) of the symbol.
        """
        self._ensure_valid_request(file_path)
        language = Language.from_file_path(file_path)
        results = []
        for server_thread in self.language_servers[language]:
            if server_thread.is_alive():
                server_thread.request_definition(file_path, line, column, results)
                server_thread.request_event.wait()  # Wait for the request to be processed
        self.notifier.log("\n".join(results))
        return "\n".join(results)

if __name__ == "__main__":
    class PrintNotifier(Notifier):
        def log(self, message: str) -> None:
            print(message)

        def status(self, status: str) -> None:
            print(status)

        def start(self) -> None:
            pass

        def stop(self) -> None:
            pass

    lsc = LanguageServerCoordinator(PrintNotifier())
    lsc.request_definition("/Users/lalvoeiro/Development/goose/src/goose/build.py", 41, 28)
    print()
