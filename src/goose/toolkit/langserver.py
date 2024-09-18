from typing import Optional
from goose.notifier import Notifier
from goose.toolkit.base import Requirements, Toolkit, tool
from exchange import Message
import threading
from rich import print

from goose.language_server.core.ls_client import LSClient


class LSPRequest:
    def __init__(self, id: int) -> None:
        self.id = id
        self.status = "pending"
        self.event = threading.Event()
        self.response = None


class LanguageServer(Toolkit):
    def __init__(self, notifier: Notifier, requires: Optional[Requirements] = None) -> None:
        super().__init__(notifier, requires)
        self.ls_client = LSClient()

    def system(self) -> str:
        """Retrieve detailed configuration and procedural guidelines for Language Server operations"""
        return Message.load("prompts/language_server.jinja").text

    def open_document(self, file_path: str) -> None:
        self.ls_client.send_and_receive_lsp(
            self.next_request_id,
            "textDocument/didOpen",
            {
                "textDocument": {
                    "uri": f"file://{file_path}",
                    "languageId": "python",
                    "version": 1,
                    "text": open(file_path).read(),
                }
            },
        )

    @tool
    def hover(self, line_number: int, character_number: int, file: str) -> str:
        """
        Get hover information for a specific position in a file

        Args:
            line_number (int): The (0-indexed) line number (e.g. 10 for the 11th line)
            character_number (int): The character number (0-indexed) (e.g. 5 for the 6th character on the line)
            file (str): The file path
        """
        self.open_document(file)
        params = {
            "textDocument": {"uri": f"file://{file}"},
            "position": {"line": line_number, "character": character_number},
        }
        # return self.request_lsp("textDocument/hover", params)


if __name__ == "__main__":
    ls = LanguageServer()
    print("hello")
    ls.open_document("/Users/lalvoeiro/Development/open-source/goose/src/goose/toolkit/langserver.py")
    # ls.hover(0, 9, "/Users/lalvoeiro/Development/open-source/goose/src/goose/toolkit/langserver.py")
    # print(ls.request_lsp("exampleMethod", {}))
