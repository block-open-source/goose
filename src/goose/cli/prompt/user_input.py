from dataclasses import dataclass
from enum import Enum
from typing import Optional


class PromptAction(Enum):
    CONTINUE = 1
    EXIT = 2


@dataclass
class UserInput:
    action: PromptAction
    text: Optional[str] = None

    def to_exit(self) -> bool:
        return self.action == PromptAction.EXIT

    def to_continue(self) -> bool:
        return self.action == PromptAction.CONTINUE
