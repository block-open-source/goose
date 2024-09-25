"""
This module contains the exceptions raised by the Language Server framework.
"""


class LanguageServerError(Exception):
    """
    Exceptions raised by the Language Server framework.
    """

    def __init__(self, message: str) -> None:
        """
        Initializes the exception with the given message.
        """
        super().__init__(message)
