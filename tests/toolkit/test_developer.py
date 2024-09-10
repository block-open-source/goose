from pathlib import Path
from tempfile import TemporaryDirectory
from unittest.mock import MagicMock, Mock

import pytest
from goose.toolkit.base import Requirements
from goose.toolkit.developer import Developer
from contextlib import contextmanager
import os


@contextmanager
def change_dir(new_dir):
    """Context manager to temporarily change the current working directory."""
    original_dir = os.getcwd()
    os.chdir(new_dir)
    try:
        yield
    finally:
        os.chdir(original_dir)


@pytest.fixture
def temp_dir():
    with TemporaryDirectory() as temp_dir:
        yield Path(temp_dir)


@pytest.fixture
def developer_toolkit():
    toolkit = Developer(notifier=MagicMock(), requires=Requirements(""))

    # This mocking ensures that that the safety check is considered a pass in shell calls
    toolkit.exchange_view = Mock()
    toolkit.exchange_view.processor.replace.return_value = Mock()
    toolkit.exchange_view.processor.replace.return_value.messages = []
    toolkit.exchange_view.processor.replace.return_value.add = Mock()
    toolkit.exchange_view.processor.replace.return_value.reply.return_value.text = "3"
    toolkit.exchange_view.processor.replace.return_value.messages = [Mock()]

    return toolkit


def test_system_prompt_with_goosehints(temp_dir, developer_toolkit):
    readme_file = temp_dir / "README.md"
    readme_file.write_text("This is from the README.md file.")

    hints_file = temp_dir / ".goosehints"
    jinja_template_content = "Hints:\n\n{% include 'README.md' %}\nEnd."
    hints_file.write_text(jinja_template_content)

    with change_dir(temp_dir):
        system_prompt = developer_toolkit.system()
        expected_end = "Hints:\n\nThis is from the README.md file.\nEnd."
        assert system_prompt.endswith(expected_end)


def test_update_plan(developer_toolkit):
    tasks = [
        {"description": "Task 1", "status": "planned"},
        {"description": "Task 2", "status": "complete"},
        {"description": "Task 3", "status": "in-progress"},
    ]
    updated_tasks = developer_toolkit.update_plan(tasks)
    assert updated_tasks == tasks


def test_patch_file(temp_dir, developer_toolkit):
    test_file = temp_dir / "test.txt"
    before_content = "Hello World"
    after_content = "Hello Goose"
    test_file.write_text(before_content)
    developer_toolkit.patch_file(test_file.as_posix(), before_content, after_content)
    assert test_file.read_text() == after_content


def test_read_file(temp_dir, developer_toolkit):
    test_file = temp_dir / "test.txt"
    content = "Hello World"
    test_file.write_text(content)
    read_content = developer_toolkit.read_file(test_file.as_posix())
    assert content in read_content


def test_shell(developer_toolkit):
    command = "echo Hello World"
    result = developer_toolkit.shell(command)
    assert "Hello World" in result


def test_write_file(temp_dir, developer_toolkit):
    test_file = temp_dir / "test.txt"
    content = "Hello World"
    developer_toolkit.write_file(test_file.as_posix(), content)
    assert test_file.read_text() == content


def test_write_file_prevent_write_if_changed(temp_dir, developer_toolkit):
    test_file = temp_dir / "test.txt"
    content = "Hello World"
    updated_content = "Hello Universe"

    # Initial write to record the timestamp
    developer_toolkit.write_file(test_file.as_posix(), content)
    developer_toolkit.read_file(test_file.as_posix())

    import time

    # Modify file externally to simulate change
    time.sleep(1)
    test_file.write_text(updated_content)

    # Try to write through toolkit and check for the raised exception
    with pytest.raises(RuntimeError, match="has been modified"):
        developer_toolkit.write_file(test_file.as_posix(), content)
    assert test_file.read_text() == updated_content
