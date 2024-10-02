import pytest
from exchange import utils
from unittest.mock import patch
from exchange.message import Message
from exchange.content import Text, ToolResult
from exchange.providers.utils import messages_to_openai_spec, encode_image


def test_encode_image():
    image_path = "tests/test_image.png"
    encoded_image = encode_image(image_path)

    # Adjust this string based on the actual initial part of your base64-encoded image.
    expected_start = "iVBORw0KGgo"
    assert encoded_image.startswith(expected_start)


def test_create_object_id() -> None:
    prefix = "test"
    object_id = utils.create_object_id(prefix)
    assert object_id.startswith(prefix + "_")
    assert len(object_id) == len(prefix) + 1 + 24  # prefix + _ + 24 chars


def test_compact() -> None:
    content = "This   is \n\n   a test"
    compacted = utils.compact(content)
    assert compacted == "This is a test"


def test_parse_docstring() -> None:
    def dummy_func(a, b, c):
        """
        This function does something.

        Args:
            a (int): The first parameter.
            b (str): The second parameter.
            c (list): The third parameter.
        """
        pass

    description, parameters = utils.parse_docstring(dummy_func)
    assert description == "This function does something."
    assert parameters == [
        {"name": "a", "annotation": "int", "description": "The first parameter."},
        {"name": "b", "annotation": "str", "description": "The second parameter."},
        {"name": "c", "annotation": "list", "description": "The third parameter."},
    ]


def test_parse_docstring_no_description() -> None:
    def dummy_func(a, b, c):
        """
        Args:
            a (int): The first parameter.
            b (str): The second parameter.
            c (list): The third parameter.
        """
        pass

    with pytest.raises(ValueError) as e:
        utils.parse_docstring(dummy_func)

    assert "Attempted to load from a function" in str(e.value)


def test_json_schema() -> None:
    def dummy_func(a: int, b: str, c: list) -> None:
        pass

    schema = utils.json_schema(dummy_func)

    assert schema == {
        "type": "object",
        "properties": {
            "a": {"type": "integer"},
            "b": {"type": "string"},
            "c": {"type": "string"},
        },
        "required": ["a", "b", "c"],
    }


def test_load_plugins() -> None:
    class DummyEntryPoint:
        def __init__(self, name, plugin):
            self.name = name
            self.plugin = plugin

        def load(self):
            return self.plugin

    with patch("exchange.utils.entry_points") as entry_points_mock:
        entry_points_mock.return_value = [
            DummyEntryPoint("plugin1", object()),
            DummyEntryPoint("plugin2", object()),
        ]

        plugins = utils.load_plugins("dummy_group")

        assert "plugin1" in plugins
        assert "plugin2" in plugins
        assert len(plugins) == 2


def test_messages_to_openai_spec():
    # Use provided test image
    png_path = "tests/test_image.png"

    # Create a list of messages as input
    messages = [
        Message(role="user", content=[Text(text="Hello, Assistant!")]),
        Message(role="assistant", content=[Text(text="Here is a text with tool usage")]),
        Message(
            role="tool",
            content=[ToolResult(tool_use_id="1", output=f'"image:{png_path}')],
        ),
    ]

    # Call the function
    output = messages_to_openai_spec(messages)

    assert "This tool result included an image that is uploaded in the next message." in str(output)
    assert "{'role': 'user', 'content': [{'type': 'image_url'" in str(output)
