import pytest
from unittest.mock import patch, MagicMock
from goose.toolkit.io import IO
import sys
from unittest import mock

sys.modules["pyautogui"] = mock.Mock()


@pytest.fixture
def io_toolkit():
    notifier = MagicMock()
    return IO(notifier)


@patch("pyautogui.moveTo")
def test_move_mouse(mock_move_to, io_toolkit):
    result = io_toolkit.move_mouse(100, 200)
    mock_move_to.assert_called_once_with(100, 200)
    assert result == "Mouse moved to (100, 200)"


@patch("pyautogui.click")
def test_click_mouse(mock_click, io_toolkit):
    result = io_toolkit.click_mouse()
    mock_click.assert_called_once()
    assert result == "Mouse clicked"


@patch("pyautogui.write")
def test_type_text(mock_write, io_toolkit):
    result = io_toolkit.type_text("Hello, World!")
    mock_write.assert_called_once_with("Hello, World!")
    assert result == "Typed text: Hello, World!"
