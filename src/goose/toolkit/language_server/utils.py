"""
This file contains various utility functions like I/O operations, handling paths, etc.
"""

import gzip
import logging
import os
from typing import Tuple
import requests
import shutil
import uuid

import platform
import subprocess
from enum import Enum

from goose.toolkit.language_server.core.exception import LangClientError
from pathlib import PurePath, Path
from goose.toolkit.language_server.logger import MultilspyLogger


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
    def read_file(logger: MultilspyLogger, file_path: str) -> str:
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
            raise LangClientError("File read failed.") from None
        logger.log(f"File read '{file_path}' failed: Unsupported encoding.", logging.ERROR)
        raise LangClientError(f"File read '{file_path}' failed: Unsupported encoding.") from None

    @staticmethod
    def download_file(logger: MultilspyLogger, url: str, target_path: str) -> None:
        """
        Downloads the file from the given URL to the given {target_path}
        """
        try:
            response = requests.get(url, stream=True, timeout=60)
            if response.status_code != 200:
                logger.log(f"Error downloading file '{url}': {response.status_code} {response.text}", logging.ERROR)
                raise LangClientError("Error downoading file.")
            with open(target_path, "wb") as f:
                shutil.copyfileobj(response.raw, f)
        except Exception as exc:
            logger.log(f"Error downloading file '{url}': {exc}", logging.ERROR)
            raise LangClientError("Error downoading file.") from None

    @staticmethod
    def download_and_extract_archive(logger: MultilspyLogger, url: str, target_path: str, archive_type: str) -> None:
        """
        Downloads the archive from the given URL having format {archive_type} and extracts it to the given {target_path}
        """
        try:
            tmp_files = []
            tmp_file_name = str(PurePath(os.path.expanduser("~"), "multilspy_tmp", uuid.uuid4().hex))
            tmp_files.append(tmp_file_name)
            os.makedirs(os.path.dirname(tmp_file_name), exist_ok=True)
            FileUtils.download_file(logger, url, tmp_file_name)
            if archive_type in ["zip", "tar", "gztar", "bztar", "xztar"]:
                assert os.path.isdir(target_path)
                shutil.unpack_archive(tmp_file_name, target_path, archive_type)
            elif archive_type == "zip.gz":
                assert os.path.isdir(target_path)
                tmp_file_name_ungzipped = tmp_file_name + ".zip"
                tmp_files.append(tmp_file_name_ungzipped)
                with gzip.open(tmp_file_name, "rb") as f_in, open(tmp_file_name_ungzipped, "wb") as f_out:
                    shutil.copyfileobj(f_in, f_out)
                shutil.unpack_archive(tmp_file_name_ungzipped, target_path, "zip")
            elif archive_type == "gz":
                with gzip.open(tmp_file_name, "rb") as f_in, open(target_path, "wb") as f_out:
                    shutil.copyfileobj(f_in, f_out)
            else:
                logger.log(f"Unknown archive type '{archive_type}' for extraction", logging.ERROR)
                raise LangClientError(f"Unknown archive type '{archive_type}'")
        except Exception as exc:
            logger.log(f"Error extracting archive '{tmp_file_name}' obtained from '{url}': {exc}", logging.ERROR)
            raise LangClientError("Error extracting archive.") from exc
        finally:
            for tmp_file_name in tmp_files:
                if os.path.exists(tmp_file_name):
                    Path.unlink(Path(tmp_file_name))


class PlatformId(str, Enum):
    """
    multilspy supported platforms
    """

    WIN_x86 = "win-x86"
    WIN_x64 = "win-x64"
    WIN_arm64 = "win-arm64"
    OSX = "osx"
    OSX_x64 = "osx-x64"
    OSX_arm64 = "osx-arm64"
    LINUX_x86 = "linux-x86"
    LINUX_x64 = "linux-x64"
    LINUX_arm64 = "linux-arm64"
    LINUX_MUSL_x64 = "linux-musl-x64"
    LINUX_MUSL_arm64 = "linux-musl-arm64"


class DotnetVersion(str, Enum):
    """
    multilspy supported dotnet versions
    """

    V4 = "4"
    V6 = "6"
    V7 = "7"
    V8 = "8"
    VMONO = "mono"


class PlatformUtils:
    """
    This class provides utilities for platform detection and identification.
    """

    @staticmethod
    def get_platform_id() -> PlatformId:
        """
        Returns the platform id for the current system
        """
        system = platform.system()
        machine = platform.machine()
        bitness = platform.architecture()[0]
        system_map = {"Windows": "win", "Darwin": "osx", "Linux": "linux"}
        machine_map = {"AMD64": "x64", "x86_64": "x64", "i386": "x86", "i686": "x86", "aarch64": "arm64"}
        if system in system_map and machine in machine_map:
            platform_id = system_map[system] + "-" + machine_map[machine]
            if system == "Linux" and bitness == "64bit":
                libc = platform.libc_ver()[0]
                if libc != "glibc":
                    platform_id += "-" + libc
            return PlatformId(platform_id)
        else:
            raise LangClientError("Unknown platform: " + system + " " + machine + " " + bitness)

    @staticmethod
    def get_dotnet_version() -> DotnetVersion:
        """
        Returns the dotnet version for the current system
        """
        try:
            result = subprocess.run(["dotnet", "--list-runtimes"], capture_output=True, check=True)
            version = ""
            for line in result.stdout.decode("utf-8").split("\n"):
                if line.startswith("Microsoft.NETCore.App"):
                    version = line.split(" ")[1]
                    break
            if version == "":
                raise LangClientError("dotnet not found on the system")
            if version.startswith("8"):
                return DotnetVersion.V8
            elif version.startswith("7"):
                return DotnetVersion.V7
            elif version.startswith("6"):
                return DotnetVersion.V6
            elif version.startswith("4"):
                return DotnetVersion.V4
            else:
                raise LangClientError("Unknown dotnet version: " + version)
        except subprocess.CalledProcessError:
            try:
                result = subprocess.run(["mono", "--version"], capture_output=True, check=True)
                return DotnetVersion.VMONO
            except subprocess.CalledProcessError:
                raise LangClientError("dotnet or mono not found on the system")
