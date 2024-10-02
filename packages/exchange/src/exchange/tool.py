import inspect
from typing import Any, Callable, Type

from attrs import define

from exchange.utils import json_schema, parse_docstring


@define
class Tool:
    """A tool that can be used by a model.

    Attributes:
        name (str): The name of the tool
        description (str): A description of what the tool does
        parameters dict[str, Any]: A json schema of the function signature
        function (Callable): The python function that powers the tool
    """

    name: str
    description: str
    parameters: dict[str, Any]
    function: Callable

    @classmethod
    def from_function(cls: Type["Tool"], func: Any) -> "Tool":  # noqa: ANN401
        """Create a tool instance from a function and its docstring

        The function must have a docstring - we require it to load the description
        and parameter descriptions. This also supports a class instance with a __call__
        method.
        """
        if inspect.isfunction(func) or inspect.ismethod(func):
            name = func.__name__
        else:
            name = func.__class__.__name__.lower()
            func = func.__call__

        description, param_descriptions = parse_docstring(func)
        schema = json_schema(func)

        # Set the 'description' field of the schema to the arg's docstring description
        for arg in param_descriptions:
            arg_name, arg_description = arg["name"], arg["description"]

            if arg_name not in schema["properties"]:
                raise ValueError(f"Argument {arg_name} found in docstring but not in schema")
            schema["properties"][arg_name]["description"] = arg_description

        return cls(
            name=name,
            description=description,
            parameters=schema,
            function=func,
        )
