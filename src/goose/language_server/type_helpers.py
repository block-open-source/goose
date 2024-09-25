"""
This module provides type-helpers used across our language server implementation
"""

import inspect

from typing import Callable, TypeVar, Type

R = TypeVar("R", bound=object)


def ensure_all_methods_implemented(
    source_cls: Type[object],
) -> Callable[[Type[R]], Type[R]]:
    """
    A decorator to ensure that all methods of source_cls class are implemented in the decorated class.
    """

    def check_all_methods_implemented(target_cls: R) -> R:
        for name, _ in inspect.getmembers(source_cls, inspect.isfunction):
            if name == "start_server":
                # we don't need to define this method in the client
                continue
            if name not in target_cls.__dict__ or not callable(target_cls.__dict__[name]):
                raise NotImplementedError(f"{name} is not implemented in {target_cls}")

        return target_cls

    return check_all_methods_implemented
