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
from langfuse.decorators import langfuse_context
import sys
from io import StringIO
from functools import cache, wraps  # Add this import

DEFAULT_LOCAL_LANGFUSE_HOST = 'http://localhost:3000'
DEFAULT_LOCAL_LANGFUSE_PUBLIC_KEY = 'publickey-local'
DEFAULT_LOCAL_LANGFUSE_SECRET_KEY =  'secretkey-local'
@cache
def auth_check() -> bool:
    # Temporarily redirect stdout and stderr to suppress print statements from Langfuse
    temp_stderr = StringIO()
    sys.stderr = temp_stderr

    # Set environment variables if not specified
    os.environ.setdefault('LANGFUSE_PUBLIC_KEY', DEFAULT_LOCAL_LANGFUSE_PUBLIC_KEY)
    os.environ.setdefault('LANGFUSE_SECRET_KEY', DEFAULT_LOCAL_LANGFUSE_SECRET_KEY)
    os.environ.setdefault('LANGFUSE_HOST', DEFAULT_LOCAL_LANGFUSE_HOST)

    auth_val = langfuse_context.auth_check()

    # Restore stderr
    sys.stderr = sys.__stderr__
    return auth_val


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
        if auth_check():

            @wraps(fn)
            def wrapped_fn(*fargs, **fkwargs):  # noqa
                return langfuse_context.observe(*args, **kwargs)(fn)(*fargs, **fkwargs)

            return wrapped_fn
        else:
            return fn

    return _wrapper
