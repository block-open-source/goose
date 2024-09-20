"""
Configuration parameters for Multilspy.
"""

from enum import Enum
from dataclasses import dataclass
from typing import Type


class Language(str, Enum):
    """
    Possible languages with Multilspy.
    """

    CSHARP = "csharp"
    PYTHON = "python"
    RUST = "rust"
    JAVA = "java"

    def __str__(self) -> str:
        return self.value


@dataclass
class MultilspyConfig:
    """
    Configuration parameters
    """

    code_language: Language
    trace_lsp_communication: bool = False

    @classmethod
    def from_dict(cls: Type["MultilspyConfig"], env: dict) -> "MultilspyConfig":
        """
        Create a MultilspyConfig instance from a dictionary
        """
        import inspect

        return cls(**{k: v for k, v in env.items() if k in inspect.signature(cls).parameters})
