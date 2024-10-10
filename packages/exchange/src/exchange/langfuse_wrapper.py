"""
Langfuse Integration Module

This module provides integration with Langfuse, a tool for monitoring and tracing LLM applications.

Usage:
    Import this module to enable Langfuse integration.
    It automatically checks for Langfuse credentials in the .env.langfuse file and for a running Langfuse server.
    If these are found, it will set up the necessary client and context for tracing.

Note:
    Run setup_langfuse.sh which automates the steps for running local Langfuse.
"""

import os
from typing import Callable
from dotenv import load_dotenv
from langfuse.decorators import langfuse_context
import sys
from io import StringIO
from pathlib import Path
from functools import wraps  # Add this import


def find_package_root(start_path: Path, marker_file="pyproject.toml") -> Path:
    while start_path != start_path.parent:
        if (start_path / marker_file).exists():
            return start_path
        start_path = start_path.parent
    return None


def auth_check() -> bool:
    # Temporarily redirect stdout and stderr to suppress print statements from Langfuse
    temp_stderr = StringIO()
    sys.stderr = temp_stderr

    # Load environment variables
    load_dotenv(LANGFUSE_ENV_FILE, override=True)

    auth_val = langfuse_context.auth_check()

    # Restore stderr
    sys.stderr = sys.__stderr__
    return auth_val


CURRENT_DIR = Path(__file__).parent
PACKAGE_ROOT = find_package_root(CURRENT_DIR)

LANGFUSE_ENV_FILE = os.path.join(PACKAGE_ROOT, ".env.langfuse.local")
HAS_LANGFUSE_CREDENTIALS = False
load_dotenv(LANGFUSE_ENV_FILE, override=True)

HAS_LANGFUSE_CREDENTIALS = auth_check()


def observe_wrapper(*args, **kwargs) -> Callable:  # noqa
    """
    A decorator that wraps a function with Langfuse context observation if credentials are available.

    If Langfuse credentials were found, the function will be wrapped with Langfuse's observe method.
    Otherwise, the function will be returned as-is.

    Args:
        *args: Positional arguments to pass to langfuse_context.observe.
        **kwargs: Keyword arguments to pass to langfuse_context.observe.

    Returns:
        Callable: The wrapped function if credentials are available, otherwise the original function.
    """

    def _wrapper(fn: Callable) -> Callable:
        if HAS_LANGFUSE_CREDENTIALS:

            @wraps(fn)
            def wrapped_fn(*fargs, **fkwargs):
                return langfuse_context.observe(*args, **kwargs)(fn)(*fargs, **fkwargs)

            return wrapped_fn
        else:
            return fn

    return _wrapper
