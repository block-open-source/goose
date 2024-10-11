"""
This module provides type-helpers used across our language server implementation

This file is obtained from https://github.com/microsoft/multilspy under the MIT License with the following terms:

MIT License

Copyright (c) Microsoft Corporation.

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE
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
