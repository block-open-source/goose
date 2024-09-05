from typing import Any, Callable, Dict, List, Type, TypeVar

T = TypeVar("T")

def ensure(cls: Type[T]) -> Callable[[Any], T]:
    """Convert dictionary to a class instance"""

    def converter(val: Any) -> T:  # noqa: ANN401
        if isinstance(val, cls):
            return val
        elif isinstance(val, dict):
            return cls(**val)
        elif isinstance(val, list):
            return cls(*val)
        else:
            return cls(val)

    return converter


def ensure_list(cls: Type[T]) -> Callable[[List[Dict[str, Any]]], Type[T]]:
    """Convert a list of dictionaries to class instances"""

    def converter(val: List[Dict[str, Any]]) -> List[T]:
        output = []
        for entry in val:
            output.append(ensure(cls)(entry))
        return output

    return converter
