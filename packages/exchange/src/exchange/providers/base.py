import os
from abc import ABC, abstractmethod
from attrs import define, field
from typing import Optional

from exchange.message import Message
from exchange.tool import Tool


@define(hash=True)
class Usage:
    input_tokens: int = field(factory=None)
    output_tokens: int = field(default=None)
    total_tokens: int = field(default=None)


class EmptyProviderNameError(Exception):
    def __init__(self, provider_cls: str) -> None:
        self.message = f"The provider class '{provider_cls}' has an empty PROVIDER_NAME."
        super().__init__(self.message)


class Provider(ABC):
    PROVIDER_NAME: str
    REQUIRED_ENV_VARS: list[str] = []

    @classmethod
    def from_env(cls: type["Provider"]) -> "Provider":
        if not cls.PROVIDER_NAME:
            raise EmptyProviderNameError(cls.__name__)
        return cls()

    @classmethod
    def check_env_vars(cls: type["Provider"], instructions_url: Optional[str] = None) -> None:
        missing_vars = [x for x in cls.REQUIRED_ENV_VARS if x not in os.environ]

        if missing_vars:
            env_vars = ", ".join(missing_vars)
            raise MissingProviderEnvVariableError(env_vars, cls.PROVIDER_NAME, instructions_url)

    @abstractmethod
    def complete(
        self,
        model: str,
        system: str,
        messages: list[Message],
        tools: tuple[Tool, ...],
        **kwargs: dict[str, any],
    ) -> tuple[Message, Usage]:
        """Generate the next message using the specified model"""
        pass


class MissingProviderEnvVariableError(Exception):
    def __init__(self, env_variable: str, provider: str, instructions_url: Optional[str] = None) -> None:
        self.env_variable = env_variable
        self.provider = provider
        self.instructions_url = instructions_url
        self.message = f"Missing environment variables: {env_variable} for provider {provider}."
        if instructions_url:
            self.message += f"\nPlease see {instructions_url} for instructions"
        super().__init__(self.message)
