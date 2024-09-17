import json
import os
import subprocess
from typing import Any
from goose.toolkit.base import tool
from exchange import Message
import threading
from rich import print

from goose.toolkit.language_server.core.logger import LanguageClientLogger


class LSClient:
    def __init__(
        self,
    ) -> None:
        self.next_request_id = 1
        self.response_map = {}
        self.response_lock = threading.Lock()
        self.logger = LanguageClientLogger()

        capabilities = {
            "textDocument": {
                "synchronization": {"openClose": True},
                "hover": {"dynamicRegistration": False, "contentFormat": ["markdown", "plaintext"]},
            }
        }
        log_file = open("pyright-langserver.log", "w")

        # Start the pyright-langserver process over stdio
        self.langserver_process = subprocess.Popen(
            ["pyright-langserver", "--stdio"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=log_file
        )

        # Create a thread to handle incoming responses
        threading.Thread(target=self._receive_responses, daemon=True).start()

        initialize_params = {
            "processId": None,
            "rootUri": f"file://{os.getcwd()}",
            "capabilities": capabilities,
            "trace": "verbose",
        }
        self.send_and_receive_lsp(self.next_request_id, "initialize", initialize_params)

    def system(self) -> str:
        """Retrieve detailed configuration and procedural guidelines for Language Server operations"""
        return Message.load("prompts/language_server.jinja").text

    def request_lsp(self, method: str, params: str) -> dict[str, Any]:
        """
        Send a message to the Language Server and receive the response

        Args:
            method (str): The method to call (e.g. "textDocument/hover")
            params (str): The parameters to pass (e.g. { "textDocument": { "uri": "file:///path/to/file" }, "position": { "line": 10, "character": 5 }})

        Returns:
            dict: The response from the Language Server
        """
        id_of_request = self.next_request_id
        return self.send_and_receive_lsp(id_of_request, method, params)

    def send_and_receive_lsp(self, request_id: int, method: str, params: dict[str, Any]) -> dict[str, Any]:
        # Create a new request object that contains the event and response
        lsp_request = LSPRequest(request_id)
        self.response_map[request_id] = lsp_request

        # Sending request
        message = {"jsonrpc": "2.0", "id": request_id, "method": method, "params": params}
        message_str = json.dumps(message)
        content_length = len(message_str)

        request = f"Content-Length: {content_length}\r\n\r\n{message_str}"
        if self.debug:
            print(f"[blue]Sending request: {request}[/]")
        self.langserver_process.stdin.write(request.encode())
        self.langserver_process.stdin.flush()

        # Incrementing the request ID
        self.next_request_id += 1

        # Wait for the event to be signaled (response received) or timeout
        if not lsp_request.event.wait(timeout=40):  # 10 seconds timeout, adjust as necessary
            raise TimeoutError(f"No response received for request ID {request_id} within the timeout period.")

        # Return the response once the event is signaled
        if self.debug:
            print(f"[green]Response received for request ID {request_id}[/]")

        return lsp_request.response

    def _receive_responses(self) -> None:
        # TODO: ensure out of order responses are handled correctly
        while True:
            headers = self.langserver_process.stdout.readline().decode().strip()
            if self.debug:
                print(f"[green]Received headers: {headers}[/]")

            if not headers.startswith("Content-Length"):
                continue  # Ignore invalid headers and continue listening

            content_length = int(headers.split(":")[1].strip()) + 2

            # Read the actual content (response)
            response = self.langserver_process.stdout.read(content_length).decode()
            if self.debug:
                print(f"[green]Received from langserver: {response.strip()}[/]")

            # Check if the response is properly formatted JSON
            try:
                json_response = json.loads(response)
                # some responses don't have an id
                response_id = json_response.get("id")
                if response_id is not None:
                    # Signal the event and pass the response
                    lsp_request = self.response_map.get(response_id)
                    if lsp_request.status == "pending":
                        lsp_request.response = json_response
                        lsp_request.event.set()  # Signal that the response has been received
            except json.JSONDecodeError:
                print(f"[red]Invalid JSON received: {response.strip()}[/]")
                continue  # Ignore invalid JSON and continue listening
            if "error" in json_response:
                print(f"[red]Error received: {json_response['error']}[/]")

    def open_document(self, file_path: str) -> None:
        self.send_and_receive_lsp(
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
        return self.request_lsp("textDocument/hover", params)
