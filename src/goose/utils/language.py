from enum import Enum
from typing import Type


class Language(str, Enum):
    """
    Possible languages supported by registered language servers.
    """

    CSHARP = "csharp"
    PYTHON = "python"
    RUST = "rust"
    JAVA = "java"
    JAVASCRIPT = "javascript"
    TYPESCRIPT = "typescript"
    CPLUSPLUS = "cpp"
    C = "c"
    RUBY = "ruby"
    GO = "go"
    PHP = "php"
    HTML = "html"
    CSS = "css"
    SHELL = "shell"
    PERL = "perl"
    SWIFT = "swift"
    KOTLIN = "kotlin"
    DART = "dart"
    R = "r"
    OBJECTIVEC = "objective-c"

    def __str__(self) -> str:
        return self.value

    @classmethod
    def from_file_path(cls: Type["Language"], file_path: str) -> "Language":
        """
        Get the language from a file path.
        """
        from pathlib import Path

        ext = Path(file_path).suffix
        match ext:
            case ".cs":
                return cls.CSHARP
            case ".py":
                return cls.PYTHON
            case ".rs":
                return cls.RUST
            case ".java":
                return cls.JAVA
            case ".js" | ".jsx":
                return cls.JAVASCRIPT
            case ".ts" | ".tsx":
                return cls.TYPESCRIPT
            case ".cpp":
                return cls.CPLUSPLUS
            case ".c":
                return cls.C
            case ".rb":
                return cls.RUBY
            case ".go":
                return cls.GO
            case ".php":
                return cls.PHP
            case ".html":
                return cls.HTML
            case ".css":
                return cls.CSS
            case ".sh":
                return cls.SHELL
            case ".pl":
                return cls.PERL
            case ".swift":
                return cls.SWIFT
            case ".kt":
                return cls.KOTLIN
            case ".dart":
                return cls.DART
            case ".r":
                return cls.R
            case ".m":
                return cls.OBJECTIVEC
            case _:
                raise ValueError(f"Unsupported language for file {file_path}")

    def comment_char(self) -> str:
        """
        Get the comment character for the language.
        """
        match self:
            case Language.CSHARP:
                return "//"
            case Language.PYTHON:
                return "#"
            case Language.RUST:
                return "//"
            case Language.JAVA:
                return "//"
            case Language.JAVASCRIPT:
                return "//"
            case Language.TYPESCRIPT:
                return "//"
            case Language.CPLUSPLUS:
                return "//"
            case Language.C:
                return "//"
            case Language.RUBY:
                return "#"
            case Language.GO:
                return "//"
            case Language.PHP:
                return "//"
            case Language.HTML:
                return "<!--"
            case Language.CSS:
                return "/*"
            case Language.SHELL:
                return "#"
            case Language.PERL:
                return "#"
            case Language.SWIFT:
                return "//"
            case Language.KOTLIN:
                return "//"
            case Language.DART:
                return "//"
            case Language.R:
                return "#"
            case Language.OBJECTIVEC:
                return "//"
            case _:
                raise ValueError("Unsupported language")
