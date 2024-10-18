import inspect
import time
from pathlib import Path
from typing import Literal

from attrs import define, field
from jinja2 import Environment, FileSystemLoader

from exchange.content import CONTENT_TYPES, Content, Text, ToolResult, ToolUse
from exchange.utils import create_object_id

Role = Literal["user", "assistant"]


def validate_role_and_content(instance: "Message", *_: any) -> None:  # noqa: ANN401
    if instance.role == "user":
        if not (instance.text or instance.tool_result):
            raise ValueError("User message must include a Text or ToolResult")
        if instance.tool_use:
            raise ValueError("User message does not support ToolUse")
    elif instance.role == "assistant":
        if not (instance.text or instance.tool_use):
            raise ValueError("Assistant message must include a Text or ToolUsage")
        if instance.tool_result:
            raise ValueError("Assistant message does not support ToolResult")


def content_converter(contents: list[dict[str, any]]) -> list[Content]:
    return [(CONTENT_TYPES[c.pop("type")](**c) if c.__class__ not in CONTENT_TYPES.values() else c) for c in contents]


@define
class Message:
    """A message to or from a language model.

    This supports several content types to extend to tool usage and (tbi) images.

    We also provide shortcuts for simplified text usage; these two are identical:
    ```
    m = Message(role='user', content=[Text(text='abcd')])
    assert m.content[0].text == 'abcd'

    m = Message.user('abcd')
    assert m.text == 'abcd'
    ```
    """

    role: Role = field(default="user")
    id: str = field(factory=lambda: str(create_object_id(prefix="msg")))
    created: int = field(factory=lambda: int(time.time()))
    content: list[Content] = field(factory=list, validator=validate_role_and_content, converter=content_converter)

    def to_dict(self) -> dict[str, any]:
        return {
            "role": self.role,
            "id": self.id,
            "created": self.created,
            "content": [item.to_dict() for item in self.content],
        }

    @property
    def text(self) -> str:
        """The text content of this message."""
        result = []
        for content in self.content:
            if isinstance(content, Text):
                result.append(content.text)
        return "\n".join(result)

    @property
    def tool_use(self) -> list[ToolUse]:
        """All tool use content of this message."""
        result = []
        for content in self.content:
            if isinstance(content, ToolUse):
                result.append(content)
        return result

    @property
    def tool_result(self) -> list[ToolResult]:
        """All tool result content of this message."""
        result = []
        for content in self.content:
            if isinstance(content, ToolResult):
                result.append(content)
        return result

    @classmethod
    def load(
        cls: type["Message"],
        filename: str,
        role: Role = "user",
        **kwargs: dict[str, any],
    ) -> "Message":
        """Load the message from filename relative to where the load is called.

        This only supports simplified content, with a single text entry

        This is meant to emulate importing code rather than a runtime filesystem. So
        if you have a directory of code that contains example.py, and example.py has
        a function that calls User.load('example.jinja'), it will look in the same
        directory as example.py for the jinja file.
        """
        frm = inspect.stack()[1]
        mod = inspect.getmodule(frm[0])

        base_path = Path(mod.__file__).parent

        env = Environment(loader=FileSystemLoader(base_path))
        template = env.get_template(filename)
        rendered_content = template.render(**kwargs)

        return cls(role=role, content=[Text(text=rendered_content)])

    @classmethod
    def user(cls: type["Message"], text: str) -> "Message":
        return cls(role="user", content=[Text(text)])

    @classmethod
    def assistant(cls: type["Message"], text: str) -> "Message":
        return cls(role="assistant", content=[Text(text)])

    @property
    def summary(self) -> str:
        return f"message:{self.role}\n" + "\n".join(content.summary for content in self.content)
