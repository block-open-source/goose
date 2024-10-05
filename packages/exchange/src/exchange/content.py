from typing import Optional

import json
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

    @property
    def summary(self) -> str:
        return "content:text\n" + self.text


@define
class ToolUse(Content):
    id: str
    name: str
    parameters: any
    is_error: bool = False
    error_message: Optional[str] = None

    @property
    def summary(self) -> str:
        return f"content:tool_use:{self.name}\nparameters:{json.dumps(self.parameters)}"


@define
class ToolResult(Content):
    tool_use_id: str
    output: str
    is_error: bool = False

    @property
    def summary(self) -> str:
        return f"content:tool_result:error={self.is_error}\noutput:{self.output}"
