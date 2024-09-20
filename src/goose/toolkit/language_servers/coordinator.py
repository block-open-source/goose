from collections import defaultdict
import logging
from goose.language_server.config import Language, MultilspyConfig
from goose.language_server.logger import MultilspyLogger
from goose.notifier import Notifier
from goose.toolkit.base import Requirements, Toolkit, tool
from exchange import Message

from goose.utils import load_plugins


class NoSupportedLanguageServerError(Exception):
    pass


class LanguageServerCoordinator(Toolkit):
    def __init__(self, notifier: Notifier, requires: Requirements) -> None:
        super().__init__(notifier=notifier, requires=requires)
        language_server_logger = MultilspyLogger()
        language_server_config = MultilspyConfig(trace_lsp_communication=False)
        # read in all available language servers
        language_servers = defaultdict(list)
        for language_server_cls in load_plugins("goose.language_server").values():
            ls = language_server_cls.from_env(env=language_server_config, logger=language_server_logger)
            for language in ls.supported_languages:
                language_servers[language].append(ls)

    def system() -> str:
        Message.load("prompts/language_server.jinja").text

    def _ensure_valid_request(self, file_path: str) -> None:
        try:
            language = Language.from_file_path(file_path)
        except ValueError:
            raise NoSupportedLanguageServerError(f"No language servers found for {file_path} is not supported by Goose")
        if language not in self.language_servers.keys():
            raise NoSupportedLanguageServerError(f"No language servers were initialized for language {language}")

    @tool
    def hover_documentation(self, absolute_file_path: str) -> str:
        self._ensure_valid_request(absolute_file_path)

        # get the language server(s) for the file
        language_servers = self.language_servers[Language.from_file_path(absolute_file_path)]
        for language_server in language_servers:
            try:
                return language_server.hover_documentation(absolute_file_path)
            except Exception as e:
                logging.error(f"Error getting hover documentation from {language_server}: {e}")
