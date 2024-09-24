from goose.toolkit.base import Toolkit, tool


class IO(Toolkit):
    """Provides tools to control mouse and keyboard inputs."""

    @tool
    def move_mouse(self, x: int, y: int) -> str:
        """
        Move the mouse cursor to the specified (x, y) coordinates.

        Args:
            x (int): The x-coordinate to move the mouse to.
            y (int): The y-coordinate to move the mouse to.

        Return:
            (str) a message indicating the mouse has been moved.
        """
        import pyautogui

        pyautogui.moveTo(x, y)
        return f"Mouse moved to ({x}, {y})"

    @tool
    def click_mouse(self) -> str:
        """
        Perform a mouse click at the current cursor position.

        Return:
            (str) a message indicating the mouse has been clicked.
        """
        import pyautogui

        pyautogui.click()
        return "Mouse clicked"

    @tool
    def type_text(self, text: str) -> str:
        """
        Type the given text using the keyboard.

        Args:
            text (str): The text to type.

        Return:
            (str) a message indicating the text has been typed.
        """
        import pyautogui

        pyautogui.write(text)
        return f"Typed text: {text}"

    # Provide any system instructions for the model
    # This can be generated dynamically, and is run at startup time
    def system(self) -> str:
        return """**You can control the mouse and keyboard using the tools provided by the IO toolkit. Please narate all the action taken back to user.**"""
