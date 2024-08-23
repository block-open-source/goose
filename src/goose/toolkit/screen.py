import subprocess
import uuid

from goose.toolkit.base import Toolkit, tool


class Screen(Toolkit):
    """Provides an instructions on when and how to work with screenshots"""

    @tool
    def take_screenshot(self) -> str:
        """
        Take a screenshot to assist the user in debugging or designing an app. They may need help with moving a screen element, or interacting in some way where you could do with seeing the screen.

        Return:
            (str) a path to the screenshot file, in the format of image: followed by the path to the file.
        """  # noqa: E501
        # Generate a random tmp filename for screenshot
        filename = f"/tmp/goose_screenshot_{uuid.uuid4().hex}.png"

        subprocess.run(["screencapture", "-x", filename])

        return f"image:{filename}"

    # Provide any system instructions for the model
    # This can be generated dynamically, and is run at startup time
    def system(self) -> str:
        return """**When the user wants you to help debug, or work on a visual design by looking at their screen, IDE or browser, call the take_screenshot and send the output from the user.**"""  # noqa: E501
