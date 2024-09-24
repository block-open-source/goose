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
    def right_click_mouse(self) -> str:
        """
        Perform a right mouse click at the current cursor position.

        Return:
            (str) a message indicating the mouse has been right-clicked.
        """
        import pyautogui

        pyautogui.click(button="right")
        return "Mouse right-clicked"

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

    @tool
    def press(self, key: str) -> str:
        """
        Press a key on the keyboard.

        Args:
            key (str): The key to press.

        Return:
            (str) a message indicating the key has been pressed.
        """
        import pyautogui

        pyautogui.press(key)
        return f"Key {key} pressed"

    @tool
    def press_while_holding(self, keys: [str], hold_key: str) -> str:
        """
        Press a key while holding another key.

        Args:
            keys ([str]): The key to press.
            hold_key (str): The key to hold while pressing the key.

        Return:
            (str) a message indicating the key has been pressed while holding another key.
        """
        import pyautogui

        with pyautogui.hold(hold_key):
            pyautogui.press(keys)
        return f"Key {keys} pressed while holding {hold_key}"

    @tool
    def scroll(self, clicks: int, x: int = None, y: int = None) -> str:
        """
        Scroll the mouse wheel.

        Args:
            clicks (int): The number of clicks to scroll.
            x (int, optional): The x-coordinate to scroll at.
            y (int, optional): The y-coordinate to scroll at.

        Return:
            (str) a message indicating the scroll action.
        """
        import pyautogui

        pyautogui.scroll(clicks, x, y)
        return f"Scrolled {clicks} clicks at ({x}, {y})"

    @tool
    def locate_on_screen(self, image: str) -> str:
        """
        Locate an image on the screen.

        Args:
            image (str): The file path to the image to locate.

        Return:
            (str) a message indicating whether the image was found and its position.
        """
        import pyautogui

        location = pyautogui.locateOnScreen(image)
        if location:
            return f"Image found at {location}"
        else:
            return "Image not found on screen"

    @tool
    def locate_all_on_screen(self, image: str) -> str:
        """
        Locate all instances of an image on the screen.

        Args:
            image (str): The file path to the image to locate.

        Return:
            (str) a message indicating the positions of all instances found.
        """
        import pyautogui

        locations = pyautogui.locateAllOnScreen(image)
        locations_list = list(locations)
        if locations_list:
            return f"Image found at {locations_list}"
        else:
            return "No instances of the image found on screen"

    # Provide any system instructions for the model
    # This can be generated dynamically, and is run at startup time
    def system(self) -> str:
        return """**You can move the mouse, click, right-click, type text, send hotkeys, scroll,
         and locate images on the screen using the tools provided by the IO toolkit.
        Please narrate all the actions taken back to user.**"""
