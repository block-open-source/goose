import attrs
from exchange.tool import Tool


def get_current_weather(location: str) -> None:
    """Get the current weather in a given location

    Args:
        location (str): The city and state, e.g. San Francisco, CA
    """
    pass


def test_load():
    tool = Tool.from_function(get_current_weather)

    expected = {
        "name": "get_current_weather",
        "description": "Get the current weather in a given location",
        "parameters": {
            "type": "object",
            "properties": {
                "location": {
                    "type": "string",
                    "description": "The city and state, e.g. San Francisco, CA",
                },
            },
            "required": ["location"],
        },
        "function": get_current_weather,
    }

    assert attrs.asdict(tool) == expected


def another_function(
    param1: int,
    param2: str,
    param3: bool,
    param4: float,
    param5: list[int],
    param6: dict[str, float],
) -> None:
    """
    This is another example function with various types

    Args:
        param1 (int): Description for param1
        param2 (str): Description for param2
        param3 (bool): Description for param3
        param4 (float): Description for param4
        param5 (list[int]): Description for param5
        param6 (dict[str, float]): Description for param6
    """
    pass


def test_load_types():
    tool = Tool.from_function(another_function)
    expected_schema = {
        "type": "object",
        "properties": {
            "param1": {"type": "integer", "description": "Description for param1"},
            "param2": {"type": "string", "description": "Description for param2"},
            "param3": {"type": "boolean", "description": "Description for param3"},
            "param4": {"type": "number", "description": "Description for param4"},
            "param5": {
                "type": "array",
                "items": {"type": "integer"},
                "description": "Description for param5",
            },
            "param6": {
                "type": "object",
                "additionalProperties": {"type": "number"},
                "description": "Description for param6",
            },
        },
        "required": ["param1", "param2", "param3", "param4", "param5", "param6"],
    }
    assert tool.parameters == expected_schema


def numpy_function(param1: int, param2: str) -> None:
    """
    This function uses numpy style docstrings

    Parameters
    ----------
    param1 : int
        Description for param1
    param2 : str
        Description for param2
    """
    pass


def test_load_numpy_style():
    tool = Tool.from_function(numpy_function)
    expected_schema = {
        "type": "object",
        "properties": {
            "param1": {"type": "integer", "description": "Description for param1"},
            "param2": {"type": "string", "description": "Description for param2"},
        },
        "required": ["param1", "param2"],
    }
    assert tool.parameters == expected_schema


def sphinx_function(param1: int, param2: str, param3: bool) -> None:
    """
    This function uses sphinx style docstrings

    :param param1: Description for param1
    :type param1: int
    :param param2: Description for param2
    :type param2: str
    :param param3: Description for param3
    :type param3: bool
    """
    pass


def test_load_sphinx_style():
    tool = Tool.from_function(sphinx_function)
    expected_schema = {
        "type": "object",
        "properties": {
            "param1": {"type": "integer", "description": "Description for param1"},
            "param2": {"type": "string", "description": "Description for param2"},
            "param3": {"type": "boolean", "description": "Description for param3"},
        },
        "required": ["param1", "param2", "param3"],
    }
    assert tool.parameters == expected_schema


class FunctionLike:
    def __init__(self, state: int) -> None:
        self.state = state

    def __call__(self, param1: int) -> int:
        """Example

        Args:
            param1 (int): Description for param1
        """
        return self.state + param1


def test_load_stateful_class():
    tool = Tool.from_function(FunctionLike(1))
    expected_schema = {
        "type": "object",
        "properties": {
            "param1": {"type": "integer", "description": "Description for param1"},
        },
        "required": ["param1"],
    }
    assert tool.parameters == expected_schema
    assert tool.function(2) == 3
