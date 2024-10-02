from typing import Any, Dict, Optional

from attrs import define, asdict


CONTENT_TYPES = {}


class Content:
    def __init_subclass__(cls, **kwargs: Dict[str, Any]) -> None:
        super().__init_subclass__(**kwargs)
        CONTENT_TYPES[cls.__name__] = cls

    def to_dict(self) -> Dict[str, Any]:
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
    parameters: Any
    is_error: bool = False
    error_message: Optional[str] = None


@define
class ToolResult(Content):
    tool_use_id: str
    output: str
    is_error: bool = False
