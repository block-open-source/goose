from typing import Optional

from attrs import define, asdict


CONTENT_TYPES = {}


class Content:
    def __init_subclass__(cls, **kwargs: dict[str, any]) -> None:
        super().__init_subclass__(**kwargs)
        CONTENT_TYPES[cls.__name__] = cls

    def to_dict(self) -> dict[str, any]:
        data = asdict(self, recurse=True)
        data["type"] = self.__class__.__name__
        return data


@define
class Text(Content):
    text: str


@define
class ToolUse(Content):
    id: str
    name: str
    parameters: any
    is_error: bool = False
    error_message: Optional[str] = None


@define
class ToolResult(Content):
    tool_use_id: str
    output: str
    is_error: bool = False
