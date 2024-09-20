"""
Provides Python specific instantiation of the LanguageServer class. Contains various configurations and settings specific to Python.
"""

import asyncio
import json
import logging
import os
import pathlib
from contextlib import asynccontextmanager
from typing import AsyncIterator, Type

from goose.language_server.logger import MultilspyLogger
from goose.language_server.base import LanguageServer
from goose.language_server.core.server import ProcessLaunchInfo
from goose.language_server.core.lsp_types import InitializeParams
from goose.language_server.config import MultilspyConfig


def build_initialize_params(config_preset: dict, repository_absolute_path: str) -> InitializeParams:
    """
    Returns the initialize params for the Jedi Language Server.
    """
    config_preset["processId"] = os.getpid()
    assert config_preset["rootPath"] == "$rootPath"
    config_preset["rootPath"] = repository_absolute_path

    assert config_preset["rootUri"] == "$rootUri"
    config_preset["rootUri"] = pathlib.Path(repository_absolute_path).as_uri()

    assert config_preset["workspaceFolders"][0]["uri"] == "$uri"
    config_preset["workspaceFolders"][0]["uri"] = pathlib.Path(repository_absolute_path).as_uri()

    assert config_preset["workspaceFolders"][0]["name"] == "$name"
    config_preset["workspaceFolders"][0]["name"] = os.path.basename(repository_absolute_path)

    return config_preset


class JediServer(LanguageServer):
    """Provides Python specific instantiation of the LanguageServer class."""

    @classmethod
    def from_env(
        cls: Type["JediServer"], config: MultilspyConfig, logger: MultilspyLogger, **kwargs: dict
    ) -> "JediServer":
        config_file_path = os.environ.get("JEDI_LANGUAGE_SERVER_CONFIG_PATH", None)
        if config_file_path:
            config_file_path = pathlib.Path(config_file_path)
        else:
            config_file_path = pathlib.Path.cwd() / ".goose-jedi-config.json"

        if not os.path.exists(config_file_path):
            raise FileNotFoundError(f"Config file not found at {config_file_path}")

        # read in the config file and provide the config object to the JediServer
        with open(config_file_path, "r") as f:
            jedi_config = json.load(f)

        initialize_params = build_initialize_params(config_preset=jedi_config, repository_absolute_path=os.getcwd())
        return cls(
            initialize_params=initialize_params,
            language_id="python",
            logger=logger,
            config=config,
            repository_root_path=os.getcwd(),
            process_launch_info=ProcessLaunchInfo(cmd="jedi-language-server", cwd=os.getcwd()),
        )

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

        async def do_nothing(_) -> None:
            return

        async def check_experimental_status(params):
            if params["quiescent"]:
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

            self.logger.log(
                "Sending initialize request from LSP client to LSP server and awaiting response",
                logging.INFO,
            )
            init_response = await self.server.send.initialize(self.initialize_params)
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
        ls = JediServer.from_env(
            MultilspyConfig(trace_lsp_communication=True),
            MultilspyLogger(),
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
