"""
This file provides the implementation of the JSON-RPC client, that launches and
communicates with the language server.

The initial implementation of this file was obtained from
https://github.com/predragnikolic/OLSP under the MIT License with the following terms:

MIT License

Copyright (c) 2023 Предраг Николић

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
"""

import asyncio
import dataclasses
import json
import os
from typing import Any, Dict, List, Optional, Union

from .lsp_requests import LspNotification, LspRequest
from .lsp_types import ErrorCodes

StringDict = Dict[str, Any]
PayloadLike = Union[List[StringDict], StringDict, None]
CONTENT_LENGTH = "Content-Length: "
ENCODING = "utf-8"


@dataclasses.dataclass
class ProcessLaunchInfo:
    """
    This class is used to store the information required to launch a process.
    """

    # The command to launch the process
    cmd: str

    # The environment variables to set for the process
    env: Dict[str, str] = dataclasses.field(default_factory=dict)

    # The working directory for the process
    cwd: str = os.getcwd()


class Error(Exception):
    def __init__(self, code: ErrorCodes, message: str) -> None:
        super().__init__(message)
        self.code = code

    def to_lsp(self) -> StringDict:
        return {"code": self.code, "message": super().__str__()}

    @classmethod
    def from_lsp(cls, d: StringDict) -> "Error":
        return Error(d["code"], d["message"])

    def __str__(self) -> str:
        return f"{super().__str__()} ({self.code})"


def make_response(request_id: Any, params: PayloadLike) -> StringDict:
    return {"jsonrpc": "2.0", "id": request_id, "result": params}


def make_error_response(request_id: Any, err: Error) -> StringDict:
    return {"jsonrpc": "2.0", "id": request_id, "error": err.to_lsp()}


def make_notification(method: str, params: PayloadLike) -> StringDict:
    return {"jsonrpc": "2.0", "method": method, "params": params}


def make_request(method: str, request_id: Any, params: PayloadLike) -> StringDict:
    return {"jsonrpc": "2.0", "method": method, "id": request_id, "params": params}


class StopLoopException(Exception):
    pass


def create_message(payload: PayloadLike):
    body = json.dumps(payload, check_circular=False, ensure_ascii=False, separators=(",", ":")).encode(ENCODING)
    return (
        f"Content-Length: {len(body)}\r\n".encode(ENCODING),
        "Content-Type: application/vscode-jsonrpc; charset=utf-8\r\n\r\n".encode(ENCODING),
        body,
    )


class MessageType:
    error = 1
    warning = 2
    info = 3
    log = 4


class Request:
    def __init__(self) -> None:
        self.cv = asyncio.Condition()
        self.result: Optional[PayloadLike] = None
        self.error: Optional[Error] = None

    async def on_result(self, params: PayloadLike) -> None:
        self.result = params
        async with self.cv:
            self.cv.notify()

    async def on_error(self, err: Error) -> None:
        self.error = err
        async with self.cv:
            self.cv.notify()


def content_length(line: bytes) -> Optional[int]:
    if line.startswith(b"Content-Length: "):
        _, value = line.split(b"Content-Length: ")
        value = value.strip()
        try:
            return int(value)
        except ValueError:
            raise ValueError("Invalid Content-Length header: {}".format(value))
    return None


class LanguageServerHandler:
    """
    This class provides the implementation of Python client for the Language Server Protocol.
    A class that launches the language server and communicates with it
    using the Language Server Protocol (LSP).

    It provides methods for sending requests, responses, and notifications to the server
    and for registering handlers for requests and notifications from the server.

    Uses JSON-RPC 2.0 for communication with the server over stdin/stdout.

    Attributes:
        send: A LspRequest object that can be used to send requests to the server and
            await for the responses.
        notify: A LspNotification object that can be used to send notifications to the server.
        cmd: A string that represents the command to launch the language server process.
        process: A subprocess.Popen object that represents the language server process.
        _received_shutdown: A boolean flag that indicates whether the client has received
            a shutdown request from the server.
        request_id: An integer that represents the next available request id for the client.
        _response_handlers: A dictionary that maps request ids to Request objects that
            store the results or errors of the requests.
        on_request_handlers: A dictionary that maps method names to callback functions
            that handle requests from the server.
        on_notification_handlers: A dictionary that maps method names to callback functions
            that handle notifications from the server.
        logger: An optional function that takes two strings (source and destination) and
            a payload dictionary, and logs the communication between the client and the server.
        tasks: A dictionary that maps task ids to asyncio.Task objects that represent
            the asynchronous tasks created by the handler.
        task_counter: An integer that represents the next available task id for the handler.
        loop: An asyncio.AbstractEventLoop object that represents the event loop used by the handler.
    """

    def __init__(self, process_launch_info: ProcessLaunchInfo, logger=None) -> None:
        """
        Params:
            cmd: A string that represents the command to launch the language server process.
            logger: An optional function that takes two strings (source and destination) and
                a payload dictionary, and logs the communication between the client and the server.
        """
        self.send = LspRequest(self.send_request)
        self.notify = LspNotification(self.send_notification)

        self.process_launch_info = process_launch_info
        self.process = None
        self._received_shutdown = False

        self.request_id = 1
        self._response_handlers: Dict[Any, Request] = {}
        self.on_request_handlers = {}
        self.on_notification_handlers = {}
        self.logger = logger
        self.tasks = {}
        self.task_counter = 0
        self.loop = None

    async def start(self) -> None:
        """
        Starts the language server process and creates a task to continuously read from its stdout to handle communications
        from the server to the client
        """
        child_proc_env = os.environ.copy()
        child_proc_env.update(self.process_launch_info.env)
        self.process = await asyncio.create_subprocess_shell(
            self.process_launch_info.cmd,
            stdout=asyncio.subprocess.PIPE,
            stdin=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
            env=child_proc_env,
            cwd=self.process_launch_info.cwd,
        )

        self.loop = asyncio.get_event_loop()
        self.tasks[self.task_counter] = self.loop.create_task(self.run_forever())
        self.task_counter += 1
        self.tasks[self.task_counter] = self.loop.create_task(self.run_forever_stderr())
        self.task_counter += 1

    async def stop(self) -> None:
        """
        Sends the terminate signal to the language server process and waits for it to exit, with a timeout, killing it if necessary
        """
        for task in self.tasks.values():
            task.cancel()

        self.tasks = {}

        process = self.process
        self.process = None

        if process:
            # TODO: Ideally, we should terminate the process here,
            # However, there's an issue with asyncio terminating processes documented at
            # https://bugs.python.org/issue35539 and https://bugs.python.org/issue41320
            # process.terminate()
            wait_for_end = process.wait()
            try:
                await asyncio.wait_for(wait_for_end, timeout=60)
            except asyncio.TimeoutError:
                process.kill()

    async def shutdown(self) -> None:
        """
        Perform the shutdown sequence for the client, including sending the shutdown request to the server and notifying it of exit
        """
        await self.send.shutdown()
        self._received_shutdown = True
        self.notify.exit()
        if self.process and self.process.stdout:
            self.process.stdout.set_exception(StopLoopException())
            # This yields the control to the event loop to allow the exception to be handled
            # in the run_forever and run_forever_stderr methods
            await asyncio.sleep(0)

    def _log(self, message: str) -> None:
        """
        Create a log message
        """
        if self.logger:
            self.logger("client", "logger", message)

    async def run_forever(self) -> bool:
        """
        Continuously read from the language server process stdout and handle the messages
        invoking the registered response and notification handlers
        """
        try:
            while self.process and self.process.stdout and not self.process.stdout.at_eof():
                line = await self.process.stdout.readline()
                if not line:
                    continue
                try:
                    num_bytes = content_length(line)
                except ValueError:
                    continue
                if num_bytes is None:
                    continue
                while line and line.strip():
                    line = await self.process.stdout.readline()
                if not line:
                    continue
                body = await self.process.stdout.readexactly(num_bytes)

                self.tasks[self.task_counter] = asyncio.get_event_loop().create_task(self._handle_body(body))
                self.task_counter += 1
        except (BrokenPipeError, ConnectionResetError, StopLoopException):
            pass
        return self._received_shutdown

    async def run_forever_stderr(self) -> None:
        """
        Continuously read from the language server process stderr and log the messages
        """
        try:
            while self.process and self.process.stderr and not self.process.stderr.at_eof():
                line = await self.process.stderr.readline()
                if not line:
                    continue
                self._log("LSP stderr: " + line.decode(ENCODING))
        except (BrokenPipeError, ConnectionResetError, StopLoopException):
            pass

    async def _handle_body(self, body: bytes) -> None:
        """
        Parse the body text received from the language server process and invoke the appropriate handler
        """
        try:
            await self._receive_payload(json.loads(body))
        except IOError as ex:
            self._log(f"malformed {ENCODING}: {ex}")
        except UnicodeDecodeError as ex:
            self._log(f"malformed {ENCODING}: {ex}")
        except json.JSONDecodeError as ex:
            self._log(f"malformed JSON: {ex}")

    async def _receive_payload(self, payload: StringDict) -> None:
        """
        Determine if the payload received from server is for a request, response, or notification and invoke the appropriate handler
        """
        if self.logger:
            self.logger("server", "client", payload)
        try:
            if "method" in payload:
                if "id" in payload:
                    await self._request_handler(payload)
                else:
                    await self._notification_handler(payload)
            elif "id" in payload:
                await self._response_handler(payload)
            else:
                self._log(f"Unknown payload type: {payload}")
        except Exception as err:
            self._log(f"Error handling server payload: {err}")

    def send_notification(self, method: str, params: Optional[dict] = None) -> None:
        """
        Send notification pertaining to the given method to the server with the given parameters
        """
        self._send_payload_sync(make_notification(method, params))

    def send_response(self, request_id: Any, params: PayloadLike) -> None:
        """
        Send response to the given request id to the server with the given parameters
        """
        self.tasks[self.task_counter] = asyncio.get_event_loop().create_task(
            self._send_payload(make_response(request_id, params))
        )
        self.task_counter += 1

    def send_error_response(self, request_id: Any, err: Error) -> None:
        """
        Send error response to the given request id to the server with the given error
        """
        self.tasks[self.task_counter] = asyncio.get_event_loop().create_task(
            self._send_payload(make_error_response(request_id, err))
        )
        self.task_counter += 1

    async def send_request(self, method: str, params: Optional[dict] = None) -> None:
        """
        Send request to the server, register the request id, and wait for the response
        """
        request = Request()
        request_id = self.request_id
        self.request_id += 1
        self._response_handlers[request_id] = request
        async with request.cv:
            await self._send_payload(make_request(method, request_id, params))
            await request.cv.wait()
        if isinstance(request.error, Error):
            raise request.error
        return request.result

    def _send_payload_sync(self, payload: StringDict) -> None:
        """
        Send the payload to the server by writing to its stdin synchronously
        """
        if not self.process or not self.process.stdin:
            return
        msg = create_message(payload)
        if self.logger:
            self.logger("client", "server", payload)
        self.process.stdin.writelines(msg)

    async def _send_payload(self, payload: StringDict) -> None:
        """
        Send the payload to the server by writing to its stdin asynchronously.
        """
        if not self.process or not self.process.stdin:
            return
        msg = create_message(payload)
        if self.logger:
            self.logger("client", "server", payload)
        self.process.stdin.writelines(msg)
        await self.process.stdin.drain()

    def on_request(self, method: str, cb) -> None:
        """
        Register the callback function to handle requests from the server to the client for the given method
        """
        self.on_request_handlers[method] = cb

    def on_notification(self, method: str, cb) -> None:
        """
        Register the callback function to handle notifications from the server to the client for the given method
        """
        self.on_notification_handlers[method] = cb

    async def _response_handler(self, response: StringDict) -> None:
        """
        Handle the response received from the server for a request, using the id to determine the request
        """
        request = self._response_handlers.pop(response["id"])
        if "result" in response and "error" not in response:
            await request.on_result(response["result"])
        elif "result" not in response and "error" in response:
            await request.on_error(Error.from_lsp(response["error"]))
        else:
            await request.on_error(Error(ErrorCodes.InvalidRequest, ""))

    async def _request_handler(self, response: StringDict) -> None:
        """
        Handle the request received from the server: call the appropriate callback function and return the result
        """
        method = response.get("method", "")
        params = response.get("params")
        request_id = response.get("id")
        handler = self.on_request_handlers.get(method)
        if not handler:
            self.send_error_response(
                request_id,
                Error(
                    ErrorCodes.MethodNotFound,
                    "method '{}' not handled on client.".format(method),
                ),
            )
            return
        try:
            self.send_response(request_id, await handler(params))
        except Error as ex:
            self.send_error_response(request_id, ex)
        except Exception as ex:
            self.send_error_response(request_id, Error(ErrorCodes.InternalError, str(ex)))

    async def _notification_handler(self, response: StringDict) -> None:
        """
        Handle the notification received from the server: call the appropriate callback function
        """
        method = response.get("method", "")
        params = response.get("params")
        handler = self.on_notification_handlers.get(method)
        if not handler:
            self._log(f"unhandled {method}")
            return
        try:
            await handler(params)
        except asyncio.CancelledError:
            return
        except Exception as ex:
            if (not self._received_shutdown) and self.logger:
                self.logger(
                    "client",
                    "logger",
                    str(
                        {
                            "type": MessageType.error,
                            "message": str(ex),
                            "method": method,
                            "params": params,
                        }
                    ),
                )
