"""
Provides Python specific instantiation of the LanguageServer class. Contains various configurations and settings specific to Python.
"""

import asyncio
import json
import logging
import os
import pathlib
from contextlib import asynccontextmanager
from typing import AsyncIterator

from goose.toolkit.language_server.logger import MultilspyLogger
from goose.toolkit.language_server.language_client import LanguageServer
from goose.toolkit.language_server.core.server import ProcessLaunchInfo
from goose.toolkit.language_server.core.lsp_types import InitializeParams
from goose.toolkit.language_server.config import MultilspyConfig


class JediServer(LanguageServer):
    """
    Provides Python specific instantiation of the LanguageServer class. Contains various configurations and settings specific to Python.
    """

    def __init__(self, config: MultilspyConfig, logger: MultilspyLogger, repository_root_path: str) -> None:
        """
        Creates a JediServer instance. This class is not meant to be instantiated directly. Use LanguageServer.create() instead.
        """
        super().__init__(
            config,
            logger,
            repository_root_path,
            ProcessLaunchInfo(cmd="jedi-language-server", cwd=repository_root_path),
            "python",
        )

    def _get_initialize_params(self, repository_absolute_path: str) -> InitializeParams:
        """
        Returns the initialize params for the Jedi Language Server.
        """
        with open(os.path.join(os.path.dirname(__file__), "initialize_params.json"), "r") as f:
            d = json.load(f)

        del d["_description"]

        d["processId"] = os.getpid()
        assert d["rootPath"] == "$rootPath"
        d["rootPath"] = repository_absolute_path

        assert d["rootUri"] == "$rootUri"
        d["rootUri"] = pathlib.Path(repository_absolute_path).as_uri()

        assert d["workspaceFolders"][0]["uri"] == "$uri"
        d["workspaceFolders"][0]["uri"] = pathlib.Path(repository_absolute_path).as_uri()

        assert d["workspaceFolders"][0]["name"] == "$name"
        d["workspaceFolders"][0]["name"] = os.path.basename(repository_absolute_path)

        return d

    @asynccontextmanager
    async def start_server(self) -> AsyncIterator["JediServer"]:
        """
        Starts the JEDI Language Server, waits for the server to be ready and yields the LanguageServer instance.

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

        async def execute_client_command_handler(params):
            return []

        async def do_nothing(params):
            return

        async def check_experimental_status(params):
            if params["quiescent"] == True:
                self.completions_available.set()

        async def window_log_message(msg):
            self.logger.log(f"LSP: window/logMessage: {msg}", logging.INFO)

        self.server.on_request("client/registerCapability", do_nothing)
        self.server.on_notification("language/status", do_nothing)
        self.server.on_notification("window/logMessage", window_log_message)
        self.server.on_request("workspace/executeClientCommand", execute_client_command_handler)
        self.server.on_notification("$/progress", do_nothing)
        self.server.on_notification("textDocument/publishDiagnostics", do_nothing)
        self.server.on_notification("language/actionableNotification", do_nothing)
        self.server.on_notification("experimental/serverStatus", check_experimental_status)

        async with super().start_server():
            self.logger.log("Starting jedi-language-server server process", logging.INFO)
            await self.server.start()
            initialize_params = self._get_initialize_params(self.repository_root_path)

            self.logger.log(
                "Sending initialize request from LSP client to LSP server and awaiting response",
                logging.INFO,
            )
            init_response = await self.server.send.initialize(initialize_params)
            assert init_response["capabilities"]["textDocumentSync"]["change"] == 2
            assert "completionProvider" in init_response["capabilities"]
            assert init_response["capabilities"]["completionProvider"] == {
                "triggerCharacters": [".", "'", '"'],
                "resolveProvider": True,
            }

            self.server.notify.initialized({})

            yield self

            await self.server.shutdown()
            await self.server.stop()


if __name__ == "__main__":

    async def run_server() -> None:
        ls = JediServer(
            MultilspyConfig(
                code_language="python",
            ),
            MultilspyLogger(),
            "/Users/lalvoeiro/Development/goose",
        )
        ls.start_server()
        async with ls.start_server():
            # LanguageServer has been initialized and ready to serve requests
            res = await ls.request_references(
                "/Users/lalvoeiro/Development/goose/src/goose/toolkit/developer.py", 19, 21
            )
            # Shutdown the LanguageServer on exit from scope
            print(res)

            res = await ls.request_definition(
                "/Users/lalvoeiro/Development/goose/src/goose/toolkit/developer.py", 26, 20
            )
            print(res)

    asyncio.run(run_server())
