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

    @classmethod
    def from_file_path(cls: Type["Language"], file_path: str) -> "Language":
        """
        Get the language from a file path.
        """
        from pathlib import Path

        ext = Path(file_path).suffix
        if ext == ".cs":
            return cls.CSHARP
        elif ext == ".py":
            return cls.PYTHON
        elif ext == ".rs":
            return cls.RUST
        elif ext == ".java":
            return cls.JAVA
        else:
            raise ValueError(f"Unsupported language for file {file_path}")


@dataclass
class MultilspyConfig:
    """
    Configuration parameters
    """

    trace_lsp_communication: bool = False

    @classmethod
    def from_dict(cls: Type["MultilspyConfig"], env: dict) -> "MultilspyConfig":
        """
        Create a MultilspyConfig instance from a dictionary
        """
        import inspect

        return cls(**{k: v for k, v in env.items() if k in inspect.signature(cls).parameters})
