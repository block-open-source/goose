import inspect
import uuid
from importlib.metadata import entry_points
from typing import Any, Callable, Dict, List, Type, get_args, get_origin

from griffe import (
    Docstring,
    DocstringSection,
    DocstringSectionParameters,
    DocstringSectionText,
)


def create_object_id(prefix: str) -> str:
    return f"{prefix}_{uuid.uuid4().hex[:24]}"


def compact(content: str) -> str:
    """Replace any amount of whitespace with a single space"""
    return " ".join(content.split())


def parse_docstring(func: Callable) -> tuple[str, List[Dict]]:
    """Get description and parameters from function docstring"""
    function_args = list(inspect.signature(func).parameters.keys())
    text = str(func.__doc__)
    docstring = Docstring(text)

    for style in ["google", "numpy", "sphinx"]:
        parsed = docstring.parse(style)

        if not _check_section_is_present(parsed, DocstringSectionText):
            continue

        if function_args and not _check_section_is_present(parsed, DocstringSectionParameters):
            continue
        break
    else:  # if we did not find a valid style in the for loop
        raise ValueError(
            f"Attempted to load from a function {func.__name__} with an invalid docstring. Parameter docs are required if the function has parameters. https://mkdocstrings.github.io/griffe/reference/docstrings/#docstrings"  # noqa: E501
        )

    description = None
    parameters = []

    for section in parsed:
        if isinstance(section, DocstringSectionText):
            description = compact(section.value)
        elif isinstance(section, DocstringSectionParameters):
            parameters = [arg.as_dict() for arg in section.value]

    docstring_args = [d["name"] for d in parameters]
    if description is None:
        raise ValueError("Docstring must include a description.")

    if not docstring_args == function_args:
        extra_docstring_args = ", ".join(sorted(set(docstring_args) - set(function_args)))
        extra_function_args = ", ".join(sorted(set(function_args) - set(docstring_args)))
        if extra_docstring_args and extra_function_args:
            raise ValueError(
                f"Docstring args must match function args: docstring had extra {extra_docstring_args}; function had extra {extra_function_args}"  # noqa: E501
            )
        elif extra_function_args:
            raise ValueError(f"Docstring args must match function args: function had extra {extra_function_args}")
        elif extra_docstring_args:
            raise ValueError(f"Docstring args must match function args: docstring had extra {extra_docstring_args}")
        else:
            raise ValueError("Docstring args must match function args")

    return description, parameters


def _check_section_is_present(
    parsed_docstring: List[DocstringSection], section_type: Type[DocstringSectionText]
) -> bool:
    for section in parsed_docstring:
        if isinstance(section, section_type):
            return True
    return False


def json_schema(func: Any) -> dict[str, Any]:  # noqa: ANN401
    """Get the json schema for a function"""
    signature = inspect.signature(func)
    parameters = signature.parameters

    schema = {
        "type": "object",
        "properties": {},
        "required": [],
    }

    for param_name, param in parameters.items():
        param_schema = {}

        if param.annotation is not inspect.Parameter.empty:
            param_schema = _map_type_to_schema(param.annotation)

        if param.default is not inspect.Parameter.empty:
            param_schema["default"] = param.default

        schema["properties"][param_name] = param_schema

        if param.default is inspect.Parameter.empty:
            schema["required"].append(param_name)

    return schema


def _map_type_to_schema(py_type: Type) -> Dict[str, Any]:  # noqa: ANN401
    origin = get_origin(py_type)
    args = get_args(py_type)

    if origin is list or origin is tuple:
        return {"type": "array", "items": _map_type_to_schema(args[0] if args else Any)}
    elif origin is dict:
        return {
            "type": "object",
            "additionalProperties": _map_type_to_schema(args[1] if len(args) > 1 else Any),
        }
    elif py_type is int:
        return {"type": "integer"}
    elif py_type is bool:
        return {"type": "boolean"}
    elif py_type is float:
        return {"type": "number"}
    elif py_type is str:
        return {"type": "string"}
    else:
        return {"type": "string"}


def load_plugins(group: str) -> dict:
    """
    Load plugins based on a specified entry point group.

    This function iterates through all entry points registered under a specified group

    Args:
        group (str): The entry point group to load plugins from. This should match the group specified
                     in the package setup where plugins are defined.

    Returns:
        dict: A dictionary where each key is the entry point name, and the value is the loaded plugin object.

    Raises:
        Exception: Propagates exceptions raised by entry point loading, which might occur if a plugin
                   is not found or if there are issues with the plugin's code.
    """
    plugins = {}
    # Access all entry points for the specified group and load each.
    for entrypoint in entry_points(group=group):
        plugin = entrypoint.load()  # Load the plugin.
        plugins[entrypoint.name] = plugin  # Store the loaded plugin in the dictionary.
    return plugins
