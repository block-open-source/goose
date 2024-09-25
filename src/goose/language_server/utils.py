"""
This file contains various utility functions like I/O operations, handling paths, etc.
"""

import logging
import os
from typing import Tuple

from enum import Enum

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
        l = 0
        c = 0
        idx = 0
        while idx < index:
            if text[idx] == "\n":
                l += 1
                c = 0
            else:
                c += 1
            idx += 1

        return l, c

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
    def get_updated_position_from_line_and_column_and_edit(l: int, c: int, text_to_be_inserted: str) -> Tuple[int, int]:
        """
        Utility function to get the position of the cursor after inserting text at a given line and column.
        """
        num_newlines_in_gen_text = text_to_be_inserted.count("\n")
        if num_newlines_in_gen_text > 0:
            l += num_newlines_in_gen_text
            c = len(text_to_be_inserted.split("\n")[-1])
        else:
            c += len(text_to_be_inserted)
        return (l, c)


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
