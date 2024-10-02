import os
import logging
from typing import Callable

HAS_LANGFUSE_CREDENTIALS = False

# logger = logging.getLogger(__name__)
logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)

try:
    from langfuse.decorators import langfuse_context

    if (
        os.environ.get("LANGFUSE_HOST")
        and os.environ.get("LANGFUSE_SECRET_KEY")
        and os.environ.get("LANGFUSE_PUBLIC_KEY")
    ):
        if langfuse_context.auth_check():
            logger.info(f"Langfuse credentials found. Find traces, if enabled, under {os.environ['LANGFUSE_HOST']}.")
            HAS_LANGFUSE_CREDENTIALS = True
except Exception:
    logger.info("Trouble finding Langfuse or Langfuse credentials.")


def observe_wrapper(*args, **kwargs) -> Callable:
    """
    A decorator that wraps a function with Langfuse context observation if credentials are available.

    If Langfuse credentials are found, the function will be wrapped with Langfuse's observe method.
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
