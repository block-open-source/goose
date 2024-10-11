"""
Language Server logger module.

This file is obtained from https://github.com/microsoft/multilspy under the MIT License with the following terms:

MIT License

Copyright (c) Microsoft Corporation.

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
SOFTWARE
"""

import inspect
import logging
from datetime import datetime

from dataclasses import dataclass


@dataclass
class LogLine:
    """
    Represents a line in the Language Server log
    """

    time: str
    level: str
    caller_file: str
    caller_name: str
    caller_line: int
    message: str

    def to_dict(self) -> dict:
        """
        Convert the log line to a dictionary
        """

        return {
            "time": self.time,
            "level": self.level,
            "caller_file": self.caller_file,
            "caller_name": self.caller_name,
            "caller_line": self.caller_line,
            "message": self.message,
        }


class LanguageServerLogger:
    """
    Logger class
    """

    def __init__(self) -> None:
        self.logger = logging.getLogger("language_server")
        self.logger.setLevel(logging.INFO)

    def log(self, debug_message: str, level: int, sanitized_error_message: str = "") -> None:
        """
        Log the debug and santized messages using the logger
        """

        debug_message = debug_message.replace("'", '"').replace("\n", " ")
        sanitized_error_message = sanitized_error_message.replace("'", '"').replace("\n", " ")

        # Collect details about the callee
        curframe = inspect.currentframe()
        calframe = inspect.getouterframes(curframe, 2)
        caller_file = calframe[1][1].split("/")[-1]
        caller_line = calframe[1][2]
        caller_name = calframe[1][3]

        # Construct the debug log line
        debug_log_line = LogLine(
            time=str(datetime.now().strftime("%Y-%m-%d %H:%M:%S")),
            level=logging.getLevelName(level),
            caller_file=caller_file,
            caller_name=caller_name,
            caller_line=caller_line,
            message=debug_message,
        )

        self.logger.log(level=level, msg=str(debug_log_line.to_dict()))
