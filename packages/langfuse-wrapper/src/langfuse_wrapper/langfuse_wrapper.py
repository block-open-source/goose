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
import logging
from typing import Callable
from dotenv import load_dotenv
from langfuse.decorators import langfuse_context
import sys
from io import StringIO

logger = logging.getLogger(__name__)
logger.setLevel(logging.INFO)
if not logger.handlers:
    handler = logging.StreamHandler()
    handler.setLevel(logging.INFO)
    formatter = logging.Formatter("%(asctime)s - %(name)s - %(levelname)s - %(message)s")
    handler.setFormatter(formatter)
    logger.addHandler(handler)

CURRENT_DIR = os.path.dirname(os.path.realpath(__file__))
PACKAGE_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(CURRENT_DIR)))

LANGFUSE_ENV_FILE = os.path.join(PACKAGE_ROOT, "langfuse-wrapper", "env", ".env.langfuse.local")

HAS_LANGFUSE_CREDENTIALS = False

# Temporarily redirect stdout and stderr to suppress print statements from Langfuse
temp_stderr = StringIO()
sys.stderr = temp_stderr

# Load environment variables
load_dotenv(LANGFUSE_ENV_FILE)

if langfuse_context.auth_check():
    HAS_LANGFUSE_CREDENTIALS = True
    logger.info("Langfuse context and credentials found.")
else:
    logger.warning(
        "Langfuse context and/or credentials not found. Please ensure that your Langfuse server is running locally \
            and that your credentials are available if you wish to run Langfuse tracing locally."
    )

# Restore stderr
sys.stderr = sys.__stderr__


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
            return langfuse_context.observe(*args, **kwargs)(fn)
        else:
            return fn

    return _wrapper
