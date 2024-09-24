"""
This file contains the main interface and the public API for multilspy.
The abstract class LanguageServer provides a factory method, creator that is
intended for creating instantiations of language specific clients.
The details of Language Specific configuration are not exposed to the user.
"""

from abc import ABC, abstractmethod
import asyncio
import dataclasses
import json
import logging
import os
import pathlib
import threading
from contextlib import asynccontextmanager, contextmanager
from goose.language_server.core.lsp_constants import LSPConstants
import goose.language_server.core.lsp_types as LSPTypes

import goose.language_server.types as multilspy_types
from goose.language_server.logger import MultilspyLogger
from goose.language_server.core.server import (
    LanguageServerHandler,
    ProcessLaunchInfo,
)
from goose.language_server.core.exception import LanguageServerError
from goose.language_server.config import MultilspyConfig, Language
from goose.language_server.utils import PathUtils, FileUtils, TextUtils
from pathlib import PurePath
from typing import AsyncIterator, Iterator, List, Dict, Type, Union, Tuple
from goose.language_server.type_helpers import ensure_all_methods_implemented


@dataclasses.dataclass
class LSPFileBuffer:
    """
    This class is used to store the contents of an open LSP file in memory.
    """

    # uri of the file
    uri: str

    # The contents of the file
    contents: str

    # The version of the file
    version: int

    # The language id of the file
    language_id: str

    # reference count of the file
    ref_count: int


class LanguageServer(ABC):
    """
    The LanguageServer class provides a language agnostic interface to the Language Server Protocol.
    It is used to communicate with Language Servers of different programming languages.
    """

    @classmethod
    @abstractmethod
    def from_env(
        cls: Type["LanguageServer"], config: MultilspyConfig, logger: MultilspyLogger, **kwargs: dict
    ) -> "LanguageServer":
        pass

    @property
    def supported_languages(self) -> List[str]:
        return []

    @asynccontextmanager
    async def start_server(self) -> AsyncIterator["LanguageServer"]:
        """
        Starts the Language Server and yields the LanguageServer instance.

        Usage:
        ```
        async with lsp.start_server():
            # LanguageServer has been initialized and ready to serve requests
            await lsp.request_definition(...)
            await lsp.request_references(...)
            # Shutdown the LanguageServer on exit from scope
        # LanguageServer has been shutdown
        ```
        """
        self.server_started = True
        yield self
        self.server_started = False

    def __init__(
        self,
        config: MultilspyConfig,
        logger: MultilspyLogger,
        repository_root_path: str,
        process_launch_info: ProcessLaunchInfo,
        language_id: str,
        initialize_params: dict = {},
    ) -> None:
        """
        Initializes a LanguageServer instance.

        Do not instantiate this class directly. Use `LanguageServer.create` method instead.

        :param config: The Multilspy configuration.
        :param logger: The logger to use.
        :param repository_root_path: The root path of the repository.
        :param cmd: Each language server has a specific command used to start the server.
                    This parameter is the command to launch the language server process.
                    The command must pass appropriate flags to the binary, so that it runs in the stdio mode,
                    as opposed to HTTP, TCP modes supported by some language servers.
        """
        if type(self) == LanguageServer:
            raise LanguageServerError(
                "LanguageServer is an abstract class and cannot be instantiated directly."
                "Use LanguageServer.from_env method instead."
            )

        self.logger = logger
        self.server_started = False
        self.repository_root_path: str = repository_root_path
        self.completions_available = asyncio.Event()
        self.initialize_params = initialize_params

        if config.trace_lsp_communication:

            def logging_fn(source: str, target: str, msg: dict) -> None:
                self.logger.log(f"LSP: {source} -> {target}: {str(msg)}", logging.DEBUG)

        else:

            def logging_fn(source: str, target: str, msg: dict) -> None:
                pass

        # cmd is obtained from the child classes, which provide the language specific command to start the language server
        # LanguageServerHandler provides the functionality to start the language server and communicate with it
        self.server: LanguageServerHandler = LanguageServerHandler(process_launch_info, logger=logging_fn)

        self.language_id = language_id
        self.open_file_buffers: Dict[str, LSPFileBuffer] = {}

    @contextmanager
    def open_file(self, relative_file_path: str) -> Iterator[None]:
        """
        Open a file in the Language Server. This is required before making any requests to the Language Server.

        :param relative_file_path: The relative path of the file to open.
        """
        if not self.server_started:
            self.logger.log(
                "open_file called before Language Server started",
                logging.ERROR,
            )
            raise LanguageServerError("Language Server not started")

        absolute_file_path = str(PurePath(self.repository_root_path, relative_file_path))
        uri = pathlib.Path(absolute_file_path).as_uri()

        if uri in self.open_file_buffers:
            assert self.open_file_buffers[uri].uri == uri
            assert self.open_file_buffers[uri].ref_count >= 1

            self.open_file_buffers[uri].ref_count += 1
            yield
            self.open_file_buffers[uri].ref_count -= 1
        else:
            contents = FileUtils.read_file(self.logger, absolute_file_path)

            version = 0
            self.open_file_buffers[uri] = LSPFileBuffer(uri, contents, version, self.language_id, 1)

            self.server.notify.did_open_text_document(
                {
                    LSPConstants.TEXT_DOCUMENT: {
                        LSPConstants.URI: uri,
                        LSPConstants.LANGUAGE_ID: self.language_id,
                        LSPConstants.VERSION: 0,
                        LSPConstants.TEXT: contents,
                    }
                }
            )
            yield
            self.open_file_buffers[uri].ref_count -= 1

        if self.open_file_buffers[uri].ref_count == 0:
            self.server.notify.did_close_text_document(
                {
                    LSPConstants.TEXT_DOCUMENT: {
                        LSPConstants.URI: uri,
                    }
                }
            )
            del self.open_file_buffers[uri]

    def insert_text_at_position(
        self, relative_file_path: str, line: int, column: int, text_to_be_inserted: str
    ) -> multilspy_types.Position:
        """
        Insert text at the given line and column in the given file and return
        the updated cursor position after inserting the text.

        :param relative_file_path: The relative path of the file to open.
        :param line: The line number at which text should be inserted.
        :param column: The column number at which text should be inserted.
        :param text_to_be_inserted: The text to insert.
        """
        if not self.server_started:
            self.logger.log(
                "insert_text_at_position called before Language Server started",
                logging.ERROR,
            )
            raise LanguageServerError("Language Server not started")

        absolute_file_path = str(PurePath(self.repository_root_path, relative_file_path))
        uri = pathlib.Path(absolute_file_path).as_uri()

        # Ensure the file is open
        assert uri in self.open_file_buffers

        file_buffer = self.open_file_buffers[uri]
        file_buffer.version += 1
        change_index = TextUtils.get_index_from_line_col(file_buffer.contents, line, column)
        file_buffer.contents = (
            file_buffer.contents[:change_index] + text_to_be_inserted + file_buffer.contents[change_index:]
        )
        self.server.notify.did_change_text_document(
            {
                LSPConstants.TEXT_DOCUMENT: {
                    LSPConstants.VERSION: file_buffer.version,
                    LSPConstants.URI: file_buffer.uri,
                },
                LSPConstants.CONTENT_CHANGES: [
                    {
                        LSPConstants.RANGE: {
                            "start": {"line": line, "character": column},
                            "end": {"line": line, "character": column},
                        },
                        "text": text_to_be_inserted,
                    }
                ],
            }
        )
        new_l, new_c = TextUtils.get_updated_position_from_line_and_column_and_edit(line, column, text_to_be_inserted)
        return multilspy_types.Position(line=new_l, character=new_c)

    def delete_text_between_positions(
        self,
        relative_file_path: str,
        start: multilspy_types.Position,
        end: multilspy_types.Position,
    ) -> str:
        """
        Delete text between the given start and end positions in the given file and return the deleted text.
        """
        if not self.server_started:
            self.logger.log(
                "insert_text_at_position called before Language Server started",
                logging.ERROR,
            )
            raise LanguageServerError("Language Server not started")

        absolute_file_path = str(PurePath(self.repository_root_path, relative_file_path))
        uri = pathlib.Path(absolute_file_path).as_uri()

        # Ensure the file is open
        assert uri in self.open_file_buffers

        file_buffer = self.open_file_buffers[uri]
        file_buffer.version += 1
        del_start_idx = TextUtils.get_index_from_line_col(file_buffer.contents, start["line"], start["character"])
        del_end_idx = TextUtils.get_index_from_line_col(file_buffer.contents, end["line"], end["character"])
        deleted_text = file_buffer.contents[del_start_idx:del_end_idx]
        file_buffer.contents = file_buffer.contents[:del_start_idx] + file_buffer.contents[del_end_idx:]
        self.server.notify.did_change_text_document(
            {
                LSPConstants.TEXT_DOCUMENT: {
                    LSPConstants.VERSION: file_buffer.version,
                    LSPConstants.URI: file_buffer.uri,
                },
                LSPConstants.CONTENT_CHANGES: [{LSPConstants.RANGE: {"start": start, "end": end}, "text": ""}],
            }
        )
        return deleted_text

    def get_open_file_text(self, relative_file_path: str) -> str:
        """
        Get the contents of the given opened file as per the Language Server.

        :param relative_file_path: The relative path of the file to open.
        """
        if not self.server_started:
            self.logger.log(
                "get_open_file_text called before Language Server started",
                logging.ERROR,
            )
            raise LanguageServerError("Language Server not started")

        absolute_file_path = str(PurePath(self.repository_root_path, relative_file_path))
        uri = pathlib.Path(absolute_file_path).as_uri()

        # Ensure the file is open
        assert uri in self.open_file_buffers

        file_buffer = self.open_file_buffers[uri]
        return file_buffer.contents

    async def request_definition(
        self, relative_file_path: str, line: int, column: int
    ) -> List[multilspy_types.Location]:
        """
        Raise a [textDocument/definition](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_definition) request to the Language Server
        for the symbol at the given line and column in the given file. Wait for the response and return the result.

        :param relative_file_path: The relative path of the file that has the symbol for which definition should be looked up
        :param line: The line number of the symbol
        :param column: The column number of the symbol

        :return List[multilspy_types.Location]: A list of locations where the symbol is defined
        """

        if not self.server_started:
            self.logger.log(
                "find_function_definition called before Language Server started",
                logging.ERROR,
            )
            raise LanguageServerError("Language Server not started")

        with self.open_file(relative_file_path):
            # sending request to the language server and waiting for response
            response = await self.server.send.definition(
                {
                    LSPConstants.TEXT_DOCUMENT: {
                        LSPConstants.URI: pathlib.Path(
                            str(PurePath(self.repository_root_path, relative_file_path))
                        ).as_uri()
                    },
                    LSPConstants.POSITION: {
                        LSPConstants.LINE: line,
                        LSPConstants.CHARACTER: column,
                    },
                }
            )

        ret: List[multilspy_types.Location] = []
        if isinstance(response, list):
            # response is either of type Location[] or LocationLink[]
            for item in response:
                assert isinstance(item, dict)
                if LSPConstants.URI in item and LSPConstants.RANGE in item:
                    new_item: multilspy_types.Location = {}
                    new_item.update(item)
                    new_item["absolutePath"] = PathUtils.uri_to_path(new_item["uri"])
                    new_item["relativePath"] = str(
                        PurePath(os.path.relpath(new_item["absolutePath"], self.repository_root_path))
                    )
                    ret.append(multilspy_types.Location(new_item))
                elif (
                    LSPConstants.ORIGIN_SELECTION_RANGE in item
                    and LSPConstants.TARGET_URI in item
                    and LSPConstants.TARGET_RANGE in item
                    and LSPConstants.TARGET_SELECTION_RANGE in item
                ):
                    new_item: multilspy_types.Location = {}
                    new_item["uri"] = item[LSPConstants.TARGET_URI]
                    new_item["absolutePath"] = PathUtils.uri_to_path(new_item["uri"])
                    new_item["relativePath"] = str(
                        PurePath(os.path.relpath(new_item["absolutePath"], self.repository_root_path))
                    )
                    new_item["range"] = item[LSPConstants.TARGET_SELECTION_RANGE]
                    ret.append(multilspy_types.Location(**new_item))
                else:
                    assert False, f"Unexpected response from Language Server: {item}"
        elif isinstance(response, dict):
            # response is of type Location
            assert LSPConstants.URI in response
            assert LSPConstants.RANGE in response

            new_item: multilspy_types.Location = {}
            new_item.update(response)
            new_item["absolutePath"] = PathUtils.uri_to_path(new_item["uri"])
            new_item["relativePath"] = str(
                PurePath(os.path.relpath(new_item["absolutePath"], self.repository_root_path))
            )
            ret.append(multilspy_types.Location(**new_item))
        else:
            assert False, f"Unexpected response from Language Server: {response}"

        return ret

    async def request_references(
        self, relative_file_path: str, line: int, column: int
    ) -> List[multilspy_types.Location]:
        """
        Raise a [textDocument/references](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_references) request to the Language Server
        to find references to the symbol at the given line and column in the given file. Wait for the response and return the result.

        :param relative_file_path: The relative path of the file that has the symbol for which references should be looked up
        :param line: The line number of the symbol
        :param column: The column number of the symbol

        :return List[multilspy_types.Location]: A list of locations where the symbol is referenced
        """

        if not self.server_started:
            self.logger.log(
                "find_all_callers_of_function called before Language Server started",
                logging.ERROR,
            )
            raise LanguageServerError("Language Server not started")

        with self.open_file(relative_file_path):
            # sending request to the language server and waiting for response
            response = await self.server.send.references(
                {
                    "context": {"includeDeclaration": False},
                    "textDocument": {
                        "uri": pathlib.Path(os.path.join(self.repository_root_path, relative_file_path)).as_uri()
                    },
                    "position": {"line": line, "character": column},
                }
            )

        ret: List[multilspy_types.Location] = []
        assert isinstance(response, list)
        for item in response:
            assert isinstance(item, dict)
            assert LSPConstants.URI in item
            assert LSPConstants.RANGE in item

            new_item: multilspy_types.Location = {}
            new_item.update(item)
            new_item["absolutePath"] = PathUtils.uri_to_path(new_item["uri"])
            new_item["relativePath"] = str(
                PurePath(os.path.relpath(new_item["absolutePath"], self.repository_root_path))
            )
            ret.append(multilspy_types.Location(**new_item))

        return ret

    async def request_completions(
        self, relative_file_path: str, line: int, column: int, allow_incomplete: bool = False
    ) -> List[multilspy_types.CompletionItem]:
        """
        Raise a [textDocument/completion](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_completion) request to the Language Server
        to find completions at the given line and column in the given file. Wait for the response and return the result.

        :param relative_file_path: The relative path of the file that has the symbol for which completions should be looked up
        :param line: The line number of the symbol
        :param column: The column number of the symbol

        :return List[multilspy_types.CompletionItem]: A list of completions
        """
        with self.open_file(relative_file_path):
            open_file_buffer = self.open_file_buffers[
                pathlib.Path(os.path.join(self.repository_root_path, relative_file_path)).as_uri()
            ]
            completion_params: LSPTypes.CompletionParams = {
                "position": {"line": line, "character": column},
                "textDocument": {"uri": open_file_buffer.uri},
                "context": {"triggerKind": LSPTypes.CompletionTriggerKind.Invoked},
            }
            response: Union[List[LSPTypes.CompletionItem], LSPTypes.CompletionList, None] = None

            num_retries = 0
            while response is None or (response["isIncomplete"] and num_retries < 30):
                await self.completions_available.wait()
                response: Union[
                    List[LSPTypes.CompletionItem], LSPTypes.CompletionList, None
                ] = await self.server.send.completion(completion_params)
                if isinstance(response, list):
                    response = {"items": response, "isIncomplete": False}
                num_retries += 1

            # TODO: Understand how to appropriately handle `isIncomplete`
            if response is None or (response["isIncomplete"] and not (allow_incomplete)):
                return []

            if "items" in response:
                response = response["items"]

            response: List[LSPTypes.CompletionItem] = response

            # TODO: Handle the case when the completion is a keyword
            items = [item for item in response if item["kind"] != LSPTypes.CompletionItemKind.Keyword]

            completions_list: List[multilspy_types.CompletionItem] = []

            for item in items:
                assert "insertText" in item or "textEdit" in item
                assert "kind" in item
                completion_item = {}
                if "detail" in item:
                    completion_item["detail"] = item["detail"]

                if "label" in item:
                    completion_item["completionText"] = item["label"]
                    completion_item["kind"] = item["kind"]
                elif "insertText" in item:
                    completion_item["completionText"] = item["insertText"]
                    completion_item["kind"] = item["kind"]
                elif "textEdit" in item and "newText" in item["textEdit"]:
                    completion_item["completionText"] = item["textEdit"]["newText"]
                    completion_item["kind"] = item["kind"]
                elif "textEdit" in item and "range" in item["textEdit"]:
                    new_dot_lineno, new_dot_colno = (
                        completion_params["position"]["line"],
                        completion_params["position"]["character"],
                    )
                    assert all(
                        (
                            item["textEdit"]["range"]["start"]["line"] == new_dot_lineno,
                            item["textEdit"]["range"]["start"]["character"] == new_dot_colno,
                            item["textEdit"]["range"]["start"]["line"] == item["textEdit"]["range"]["end"]["line"],
                            item["textEdit"]["range"]["start"]["character"]
                            == item["textEdit"]["range"]["end"]["character"],
                        )
                    )

                    completion_item["completionText"] = item["textEdit"]["newText"]
                    completion_item["kind"] = item["kind"]
                elif "textEdit" in item and "insert" in item["textEdit"]:
                    assert False
                else:
                    assert False

                completion_item = multilspy_types.CompletionItem(**completion_item)
                completions_list.append(completion_item)

            return [
                json.loads(json_repr)
                for json_repr in set([json.dumps(item, sort_keys=True) for item in completions_list])
            ]

    async def request_document_symbols(
        self, relative_file_path: str
    ) -> Tuple[List[multilspy_types.UnifiedSymbolInformation], Union[List[multilspy_types.TreeRepr], None]]:
        """
        Raise a [textDocument/documentSymbol](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_documentSymbol) request to the Language Server
        to find symbols in the given file. Wait for the response and return the result.

        :param relative_file_path: The relative path of the file that has the symbols

        :return Tuple[List[multilspy_types.UnifiedSymbolInformation], Union[List[multilspy_types.TreeRepr], None]]: A list of symbols in the file, and the tree representation of the symbols
        """
        with self.open_file(relative_file_path):
            response = await self.server.send.document_symbol(
                {
                    "textDocument": {
                        "uri": pathlib.Path(os.path.join(self.repository_root_path, relative_file_path)).as_uri()
                    }
                }
            )

        ret: List[multilspy_types.UnifiedSymbolInformation] = []
        l_tree = None
        assert isinstance(response, list)
        for item in response:
            assert isinstance(item, dict)
            assert LSPConstants.NAME in item
            assert LSPConstants.KIND in item

            if LSPConstants.CHILDREN in item:
                # TODO: l_tree should be a list of TreeRepr. Define the following function to return TreeRepr as well

                def visit_tree_nodes_and_build_tree_repr(
                    tree: LSPTypes.DocumentSymbol,
                ) -> List[multilspy_types.UnifiedSymbolInformation]:
                    l: List[multilspy_types.UnifiedSymbolInformation] = []
                    children = tree["children"] if "children" in tree else []
                    if "children" in tree:
                        del tree["children"]
                    l.append(multilspy_types.UnifiedSymbolInformation(**tree))
                    for child in children:
                        l.extend(visit_tree_nodes_and_build_tree_repr(child))
                    return l

                ret.extend(visit_tree_nodes_and_build_tree_repr(item))
            else:
                ret.append(multilspy_types.UnifiedSymbolInformation(**item))

        return ret, l_tree

    async def request_hover(
        self, relative_file_path: str, line: int, column: int
    ) -> Union[multilspy_types.Hover, None]:
        """
        Raise a [textDocument/hover](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_hover) request to the Language Server
        to find the hover information at the given line and column in the given file. Wait for the response and return the result.

        :param relative_file_path: The relative path of the file that has the hover information
        :param line: The line number of the symbol
        :param column: The column number of the symbol

        :return None
        """
        with self.open_file(relative_file_path):
            response = await self.server.send.hover(
                {
                    "textDocument": {
                        "uri": pathlib.Path(os.path.join(self.repository_root_path, relative_file_path)).as_uri()
                    },
                    "position": {
                        "line": line,
                        "character": column,
                    },
                }
            )

        if response is None:
            return None

        assert isinstance(response, dict)

        return multilspy_types.Hover(**response)


class SyncLanguageServer:
    """
    The SyncLanguageServer class provides a language-agnostic interface to the Language Server Protocol.
    It communicates with Language Servers for different programming languages.
    """

    def __init__(self, language_servers: Dict[str, LanguageServer]) -> None:
        """
        Initialize SyncLanguageServer with a dictionary of language servers.
        Each language server is run on its own daemon thread.

        :param language_servers: A dictionary of {language_name: LanguageServer}
        """
        self.language_servers = language_servers
        self.server_loops = {}
        self.loop_threads = {}

        for language_name, server in self.language_servers.items():
            loop = asyncio.new_event_loop()
            thread = threading.Thread(target=loop.run_forever, daemon=True)
            self.server_loops[language_name] = loop
            self.loop_threads[language_name] = thread
            thread.start()

    @contextmanager
    def start_servers(self) -> Iterator["SyncLanguageServer"]:
        """
        Starts all language server processes and connects to them.
        Each server is run on its own thread and event loop.

        :yield: The SyncLanguageServer instance with all servers started.
        """
        ctxs = {}
        # Start all language servers
        for language_name, language_server in self.language_servers.items():
            loop = self.server_loops[language_name]
            ctx = language_server.start_server()
            ctxs[language_name] = ctx
            asyncio.run_coroutine_threadsafe(ctx.__aenter__(), loop=loop).result()

        # Yield the context for using the servers
        yield self

        # Stop all language servers and shut down their loops
        for language_name, language_server in self.language_servers.items():
            loop = self.server_loops[language_name]
            ctx = ctxs[language_name]
            asyncio.run_coroutine_threadsafe(ctx.__aexit__(None, None, None), loop=loop).result()

            # Stop the event loop and join the thread
            loop.call_soon_threadsafe(loop.stop)
            self.loop_threads[language_name].join()

    def request_definition(
        self, language_name: str, file_path: str, line: int, column: int
    ) -> List[multilspy_types.Location]:
        """
        Request definition from a specific language server.

        :param language_name: The name of the language server.
        :param file_path: The relative file path.
        :param line: The line number.
        :param column: The column number.
        :return: A list of locations where the symbol is defined.
        """
        loop = self.server_loops[language_name]
        language_server = self.language_servers[language_name]
        result = asyncio.run_coroutine_threadsafe(
            language_server.request_definition(file_path, line, column), loop
        ).result(timeout=5)
        return result
