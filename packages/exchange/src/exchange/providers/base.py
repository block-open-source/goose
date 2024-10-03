from abc import ABC, abstractmethod
from attrs import define, field
from typing import List, Optional, Tuple, Type

from exchange.message import Message
from exchange.tool import Tool


@define(hash=True)
class Usage:
    input_tokens: int = field(factory=None)
    output_tokens: int = field(default=None)
    total_tokens: int = field(default=None)


class Provider(ABC):
    @classmethod
    def from_env(cls: Type["Provider"]) -> "Provider":
        return cls()

    @abstractmethod
    def complete(
        self,
        model: str,
        system: str,
        messages: List[Message],
        tools: Tuple[Tool],
    ) -> Tuple[Message, Usage]:
        """Generate the next message using the specified model"""
        pass


class MissingProviderEnvVariableError(Exception):
    def __init__(self, env_variable: str, provider: str, instructions_url: Optional[str] = None) -> None:
        self.env_variable = env_variable
        self.provider = provider
        self.instructions_url = instructions_url
        self.message = f"Missing environment variable: {env_variable} for provider {provider}."
        if instructions_url:
            self.message += f"\n Please see {instructions_url} for instructions"
        super().__init__(self.message)
