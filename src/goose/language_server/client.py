import asyncio
from collections import defaultdict
import threading
from contextlib import ExitStack, asynccontextmanager, contextmanager
import concurrent.futures


from goose.language_server.base import LanguageServer
from goose.language_server.type_helpers import ensure_all_methods_implemented
from goose.language_server import language_server_types
from goose.utils.language import Language
from typing import Any, AsyncIterator, Callable, Iterator, List, Tuple, TypeVar, Union

T = TypeVar("T")


def language_server_request(func: Callable[[T, Any], Any]) -> Callable[[T, Any], Any]:
    def wrapper(self: "SyncLanguageServerClient", file_path: str, line: int, column: int) -> List[T]:
        language = Language.from_file_path(file_path)
        if language not in self.language_servers.keys():
            raise ValueError(f"Unsupported language for file {file_path}")
        for language_server in self.language_servers[language]:
            language_server_name = language_server.__class__.__name__
            loop = self.client_loops[language_server_name]
            result = asyncio.run_coroutine_threadsafe(
                func(self, language_server, file_path, line, column), loop
            ).result(timeout=5)
            if result:
                return result

    return wrapper


@ensure_all_methods_implemented(LanguageServer)
class SyncLanguageServerClient:
    """
    The SyncLanguageServerClient class provides a language-agnostic interface to locally runnin
    Language Servers for different programming languages. It is implemented as a singleton,
    as we only support running one instance of each programming language.
    """

    def __init__(self) -> None:
        """
        Initialize SyncLanguageServerClient with a dictionary of language servers.
        Each language server is run on its own daemon thread.
        """
        self.language_servers = defaultdict(
            list
        )  # e.g. { language (Language.PYTHON): servers[JediServer, PyrightServer, etc.]}
        self.client_loops = {}  # e.g. { "JediServer": loop, "PyrightServer": loop, etc.}
        self.server_threads = {}  # e.g. { "JediServer": thread, "PyrightServer": thread, etc.}

    def register_language_server(self, language_server: LanguageServer) -> None:
        # assert that lang servers doesnt contain an instance of this language server
        for existing_lang_server_list in self.language_servers.values():
            for existing_lang_server in existing_lang_server_list:
                assert existing_lang_server.__class__ != language_server.__class__

        supported_languages = language_server.supported_languages
        for language in supported_languages:
            self.language_servers[language].append(language_server)

        loop = asyncio.new_event_loop()
        thread = threading.Thread(target=loop.run_forever, daemon=True)
        self.client_loops[language_server.__class__.__name__] = loop
        self.server_threads[language_server.__class__.__name__] = thread
        thread.start()

    @contextmanager
    def start_servers(self) -> Iterator["SyncLanguageServerClient"]:
        """
        Starts all language server processes and connects to them.
        Each server is run on its own thread and event loop.

        :yield: The LanguageServerClient instance with all servers started.
        """
        ctxs = {}
        # Start all language servers
        for _, language_servers in self.language_servers.items():
            for language_server in language_servers:
                language_server_name = language_server.__class__.__name__
                loop = self.client_loops[language_server_name]
                ctx = language_server.start_server()
                ctxs[language_server_name] = ctx
                asyncio.run_coroutine_threadsafe(ctx.__aenter__(), loop=loop).result()  # enter the context

        # Yield the context for using the servers
        yield self

        # Stop all language servers and shut down their loops
        for language_name, _ in ctxs.items():
            loop = self.client_loops[language_name]
            ctx = ctxs[language_name]
            asyncio.run_coroutine_threadsafe(ctx.__aexit__(None, None, None), loop=loop).result()  # exit the context

            # Stop the event loop and join the thread
            loop.call_soon_threadsafe(loop.stop)
            self.server_threads[language_name].join()

    @language_server_request
    def request_definition(
        self, language_server: LanguageServer, file_path: str, line: int, column: int
    ) -> List[language_server_types.Location]:
        """
        Request definition from a specific language server.

        Args:
            file_path (str): The absolute file path.
            line (int): The line number.
            column (str): The column number.

        Return:
            (list) A list of locations where the symbol is defined.
        """
        return language_server.request_definition(file_path, line, column)

    @language_server_request
    def request_references(
        self, language_server: LanguageServer, file_path: str, line: int, column: int
    ) -> List[language_server_types.Location]:
        """
        Request references from a specific language server.
        Args:
            file_path (str): The absolute file path.
            line (int): The line number.
            column (str): The column number.
        Return:
            (list) A list of locations where the symbol is referenced.
        """
        return language_server.request_references(file_path, line, column)

    @language_server_request
    def request_hover(
        self, language_server: LanguageServer, file_path: str, line: int, column: int
    ) -> Union[language_server_types.Hover, None]:
        """
        Request hover information from a specific language server.
        Args:
            file_path (str): The absolute file path.
            line (int): The line number.
            column (str): The column number.
        Return:
            (Hover) The hover information.
        """
        return language_server.hover(file_path, line, column)

    @language_server_request
    def request_document_symbols(
        self, language_server: LanguageServer, file_path: str
    ) -> Tuple[List[language_server_types.UnifiedSymbolInformation], Union[List[language_server_types.TreeRepr], None]]:
        """
        Request document symbols from a specific language server.
        Args:
            file_path (str): The absolute file path.

        Return:
            (list) A list of document symbols.
        """
        return language_server.request_document_symbols(file_path)

    @contextmanager
    def open_file(self, file_path: str) -> Iterator[None]:
        """
        Open a file in the Language Server. This is required before making any requests to the Language Server.

        Args:
            file_path (str): The absolute path of the file to open.
        """
        language = Language.from_file_path(file_path)
        language_servers = [language_server for language_server in self.language_servers[language]]
        with ExitStack() as stack:
            [stack.open_file(file_path) for ls in language_servers]
            yield

    def notify_file_changed(self, file_path: str) -> None:
        pass
