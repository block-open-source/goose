"""
This file contains various LSP related utility functions like I/O operations, handling paths, etc.

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

import logging
import os
from typing import Tuple

from goose.language_server.core.exception import LanguageServerError
from goose.language_server.logger import LanguageServerLogger


class TextUtils:
    """
    Utilities for text operations.
    """

    @staticmethod
    def get_line_col_from_index(text: str, index: int) -> Tuple[int, int]:
        """
        Returns the zero-indexed line and column number of the given index in the given text
        """
        line_num = 0
        col_num = 0
        idx = 0
        while idx < index:
            if text[idx] == "\n":
                line_num += 1
                col_num = 0
            else:
                col_num += 1
            idx += 1

        return line_num, col_num

    @staticmethod
    def get_index_from_line_col(text: str, line: int, col: int) -> int:
        """
        Returns the index of the given zero-indexed line and column number in the given text
        """
        idx = 0
        while line > 0:
            assert idx < len(text), (idx, len(text), text)
            if text[idx] == "\n":
                line -= 1
            idx += 1
        idx += col
        return idx

    @staticmethod
    def get_updated_position_from_line_and_column_and_edit(
        line_num: int, col_num: int, text_to_be_inserted: str
    ) -> Tuple[int, int]:
        """
        Utility function to get the position of the cursor after inserting text at a given line and column.
        """
        num_newlines_in_gen_text = text_to_be_inserted.count("\n")
        if num_newlines_in_gen_text > 0:
            line_num += num_newlines_in_gen_text
            col_num = len(text_to_be_inserted.split("\n")[-1])
        else:
            col_num += len(text_to_be_inserted)
        return (line_num, col_num)


class PathUtils:
    """
    Utilities for platform-agnostic path operations.
    """

    @staticmethod
    def uri_to_path(uri: str) -> str:
        """
        Converts a URI to a file path. Works on both Linux and Windows.

        This method was obtained from https://stackoverflow.com/a/61922504
        """
        try:
            from urllib.parse import urlparse, unquote
            from urllib.request import url2pathname
        except ImportError:
            # backwards compatability
            from urlparse import urlparse
            from urllib import unquote, url2pathname
        parsed = urlparse(uri)
        host = "{0}{0}{mnt}{0}".format(os.path.sep, mnt=parsed.netloc)
        return os.path.normpath(os.path.join(host, url2pathname(unquote(parsed.path))))


class FileUtils:
    """
    Utility functions for file operations.
    """

    @staticmethod
    def read_file(logger: LanguageServerLogger, file_path: str) -> str:
        """
        Reads the file at the given path and returns the contents as a string.
        """
        encodings = ["utf-8-sig", "utf-16"]
        try:
            for encoding in encodings:
                try:
                    with open(file_path, "r", encoding=encoding) as inp_file:
                        return inp_file.read()
                except UnicodeError:
                    continue
        except Exception as exc:
            logger.log(f"File read '{file_path}' failed: {exc}", logging.ERROR)
            raise LanguageServerError("File read failed.") from None
        logger.log(f"File read '{file_path}' failed: Unsupported encoding.", logging.ERROR)
        raise LanguageServerError(f"File read '{file_path}' failed: Unsupported encoding.") from None
