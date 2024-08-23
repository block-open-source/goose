import inspect
from abc import ABC
from typing import Callable, Mapping, Optional, Tuple, TypeVar

from attrs import define, field
from exchange import Tool

from goose.notifier import Notifier

# Create a type variable that can represent any function signature
F = TypeVar("F", bound=Callable)


def tool(func: F) -> F:
    func._is_tool = True
    return func


@define
class Requirements:
    """A collection of requirements for advanced toolkits

    Requirements are an advanced use case, most toolkits will not need to
    use these. They allow one toolkit to interact with another's state.
    """

    _toolkit: str
    _requirements: Mapping[str, "Toolkit"] = field(factory=dict)

    def get(self, requirement: str) -> "Toolkit":
        """Get a requirement by name."""
        if requirement not in self._requirements:
            raise RuntimeError(
                f"The toolkit '{self._toolkit}' requested a requirement '{requirement}' but none was passed!\n"
                + f"  Make sure to include `requires: {{{requirement}: ...}}` in your profile config\n"
                + f"  See the documentation for {self._toolkit} for more details"
            )
        return self._requirements[requirement]


class Toolkit(ABC):
    """A collection of tools with corresponding prompting

    This class defines the interface that all toolkit implementations must follow,
    providing a system prompt and a collection of tools. Both are allowed to be
    empty if they are not required for the toolkit.
    """

    def __init__(self, notifier: Notifier, requires: Optional[Requirements] = None) -> None:
        self.notifier = notifier
        # This needs to be updated after the fact via build_exchange
        self.exchange_view = None

    def system(self) -> str:
        """Get the addition to the system prompt for this toolkit."""
        return ""

    def tools(self) -> Tuple[Tool, ...]:
        """Get the tools for this toolkit

        This default method looks for functions on the toolkit annotated
        with @tool.
        """
        candidates = inspect.getmembers(self, predicate=inspect.ismethod)
        return (Tool.from_function(candidate) for _, candidate in candidates if getattr(candidate, "_is_tool", None))
