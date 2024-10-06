import subprocess
from pathlib import Path
import pytest

from exchange.message import Message
from exchange.content import Text, ToolUse, ToolResult


def test_user_message():
    user_message = Message.user("abcd")
    assert user_message.role == "user"
    assert user_message.text == "abcd"


def test_assistant_message():
    assistant_message = Message.assistant("abcd")
    assert assistant_message.role == "assistant"
    assert assistant_message.text == "abcd"


def test_message_tool_use():
    from exchange.content import ToolUse

    tu1 = ToolUse(id="1", name="tool", parameters={})
    tu2 = ToolUse(id="2", name="tool", parameters={})
    message = Message(role="assistant", content=[tu1, tu2])
    assert len(message.tool_use) == 2
    assert message.tool_use[0].name == "tool"


def test_message_tool_result():
    from exchange.content import ToolResult

    tr1 = ToolResult(tool_use_id="1", output="result")
    tr2 = ToolResult(tool_use_id="2", output="result")
    message = Message(role="user", content=[tr1, tr2])
    assert len(message.tool_result) == 2
    assert message.tool_result[0].output == "result"


def test_message_load(tmpdir):
    # To emulate the expected relative lookup, we need to create a mock code dir
    # and run the load in a subprocess
    test_dir = Path(tmpdir)

    # Create a temporary Jinja template file in the test_dir
    template_content = "hello {{ name }} {% include 'relative.jinja' %}"
    template_path = test_dir / "template.jinja"
    template_path.write_text(template_content)

    relative_content = "and {{ name2 }}"
    relative_path = test_dir / "relative.jinja"
    relative_path.write_text(relative_content)

    # Create a temporary Python file in the sub_dir that calls the load method with a relative path
    python_file_content = """
from exchange.message import Message

def test_function():
    message = Message.load('template.jinja', name="a", name2="b")
    assert message.text == "hello a and b"
    assert message.role == "user"

test_function()
"""
    python_file_path = test_dir / "test_script.py"
    python_file_path.write_text(python_file_content)

    # Execute the temporary Python file to test the relative lookup functionality
    result = subprocess.run(["python3", str(python_file_path)])

    assert result.returncode == 0


def test_message_validation():
    # Valid user message
    message = Message(role="user", content=[Text(text="Hello")])
    assert message.text == "Hello"

    # Valid assistant message
    message = Message(role="assistant", content=[Text(text="Hello")])
    assert message.text == "Hello"

    # Invalid message: user with tool_use
    with pytest.raises(ValueError):
        Message(
            role="user",
            content=[Text(text=""), ToolUse(id="1", name="tool", parameters={})],
        )

    # Invalid message: assistant with tool_result
    with pytest.raises(ValueError):
        Message(
            role="assistant",
            content=[Text(text=""), ToolResult(tool_use_id="1", output="result")],
        )
