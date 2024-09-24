from collections import defaultdict
import threading
from typing import Optional
from goose.language_server.base import SyncLanguageServer
from goose.language_server.config import Language, MultilspyConfig
from goose.language_server.logger import MultilspyLogger
from goose.notifier import Notifier
from goose.toolkit.base import Requirements, Toolkit, tool
from exchange import Message

from goose.utils import load_plugins


class NoSupportedLanguageServerError(Exception):
    pass


class LanguageServerCoordinator(Toolkit):
    def __init__(self, notifier: Notifier, _: Optional[Requirements] = None) -> None:
        super().__init__(notifier=notifier, requires=_)
        language_server_logger = MultilspyLogger()
        language_server_config = MultilspyConfig(trace_lsp_communication=False)

        # read in all available language servers
        self.language_servers = defaultdict(list)
        ls = load_plugins("goose.language_server")
        self._stop_event = threading.Event()
        for language_server_cls in load_plugins("goose.language_server").values():
            ls = language_server_cls.from_env(config=language_server_config, logger=language_server_logger)
            languages = ls.supported_languages
            sync_ls = SyncLanguageServer(ls)
            for language in languages:
                self.language_servers[language].append(sync_ls)
            threading.Thread(target=self.run_server, args=(sync_ls,), daemon=True).start()

    def run_server(self, server: SyncLanguageServer) -> None:
        """
        Run the language server in its own thread.
        This method will keep the server running indefinitely.
        """
        try:
            with server.start_server() as _:
                while not self._stop_event.is_set():
                    # Keep the server running
                    self._stop_event.wait(timeout=1)
        except Exception as e:
            self.notifier.log(f"Exception in thread: {e}")

    def system(self) -> str:
        return Message.load("prompts/language_server.jinja").text

    def _ensure_valid_request(self, file_path: str) -> None:
        try:
            language = Language.from_file_path(file_path)
        except ValueError:
            raise NoSupportedLanguageServerError(f"No language servers found for {file_path} is not supported by Goose")
        if language not in self.language_servers.keys():
            raise NoSupportedLanguageServerError(f"No language servers were initialized for language {language}")

    def stop_servers(self):
        """
        Stop all language servers gracefully.
        """
        self._stop_event.set()

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
        for server in self.language_servers[language]:
            results.append(server.request_definition(file_path, line, column))
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
    try:
        lsc.request_definition("/Users/lalvoeiro/Development/goose/src/goose/build.py", 41, 28)
    finally:
        lsc.stop_servers()
