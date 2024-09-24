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


# test_press_key
@patch("pyautogui.press")
def test_press_key(mock_press, io_toolkit):
    result = io_toolkit.press("a")
    mock_press.assert_called_once_with("a")
    assert result == "Key a pressed"


@patch("pyautogui.click")
def test_right_click_mouse(mock_click, io_toolkit):
    result = io_toolkit.right_click_mouse()
    mock_click.assert_called_once_with(button="right")
    assert result == "Mouse right-clicked"


@patch("pyautogui.write")
def test_type_text(mock_write, io_toolkit):
    result = io_toolkit.type_text("Hello, World!")
    mock_write.assert_called_once_with("Hello, World!")
    assert result == "Typed text: Hello, World!"


@patch("pyautogui.scroll")
def test_scroll(mock_scroll, io_toolkit):
    result = io_toolkit.scroll(10)
    mock_scroll.assert_called_once_with(10, None, None)
    assert result == "Scrolled 10 clicks at (None, None)"


@patch("pyautogui.locateOnScreen")
def test_locate_on_screen(mock_locate_on_screen, io_toolkit):
    mock_locate_on_screen.return_value = (100, 100, 50, 50)
    result = io_toolkit.locate_on_screen("image.png")
    mock_locate_on_screen.assert_called_once_with("image.png")
    assert result == "Image found at (100, 100, 50, 50)"

    mock_locate_on_screen.return_value = None
    result = io_toolkit.locate_on_screen("image.png")
    assert result == "Image not found on screen"


@patch("pyautogui.locateAllOnScreen")
def test_locate_all_on_screen(mock_locate_all_on_screen, io_toolkit):
    mock_locate_all_on_screen.return_value = [(100, 100, 50, 50), (200, 200, 50, 50)]
    result = io_toolkit.locate_all_on_screen("image.png")
    mock_locate_all_on_screen.assert_called_once_with("image.png")
    assert result == "Image found at [(100, 100, 50, 50), (200, 200, 50, 50)]"

    mock_locate_all_on_screen.return_value = []
    result = io_toolkit.locate_all_on_screen("image.png")
    assert result == "No instances of the image found on screen"
