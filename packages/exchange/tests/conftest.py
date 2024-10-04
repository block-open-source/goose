import pytest

from exchange.providers.base import Usage


@pytest.fixture
def dummy_tool():
    def _dummy_tool() -> str:
        """An example tool"""
        return "dummy response"

    return _dummy_tool


@pytest.fixture
def usage_factory():
    def _create_usage(input_tokens=100, output_tokens=200, total_tokens=300):
        return Usage(input_tokens=input_tokens, output_tokens=output_tokens, total_tokens=total_tokens)

    return _create_usage


def read_file(filename: str) -> str:
    """
    Read the contents of the file.

    Args:
        filename (str): The path to the file, which can be relative or
            absolute. If it is a plain filename, it is assumed to be in the
            current working directory.

    Returns:
        str: The contents of the file.
    """
    assert filename == "test.txt"
    return "hello exchange"
