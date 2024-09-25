"""
Configuration parameters for a language server.
"""

from dataclasses import dataclass
from typing import Type


@dataclass
class LanguageServerConfig:
    """
    Configuration parameters
    """

    trace_lsp_communication: bool = False

    @classmethod
    def from_dict(cls: Type["LanguageServerConfig"], env: dict) -> "LanguageServerConfig":
        """
        Create a LanguageServerConfig instance from a dictionary
        """
        import inspect

        return cls(**{k: v for k, v in env.items() if k in inspect.signature(cls).parameters})
