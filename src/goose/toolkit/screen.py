import subprocess
import uuid

from rich.markdown import Markdown
from rich.panel import Panel

from goose.toolkit.base import Toolkit, tool


class Screen(Toolkit):
    """Provides an instructions on when and how to work with screenshots"""

    @tool
    def take_screenshot(self, display: int = 1) -> str:
        """
        Take a screenshot to assist the user in debugging or designing an app. They may need
        help with moving a screen element, or interacting in some way where you could do with
        seeing the screen.

        Args:
            display (int): Display to take the screen shot in. Default is the main display (1). Must be a value greater than 1.
        """  # noqa: E501
        # Generate a random tmp filename for screenshot
        filename = f"/tmp/goose_screenshot_{uuid.uuid4().hex}.jpg"
        screen_capture_command = ["screencapture", "-x", "-D", str(display), filename, "-f", "jpg"]

        subprocess.run(screen_capture_command, check=True, capture_output=True)

        resize_command = ["sips", "--resampleWidth", "768", filename, "-s", "format", "jpeg"]
        subprocess.run(resize_command, check=True, capture_output=True)

        self.notifier.log(
            Panel.fit(
                Markdown(f"```bash\n{' '.join(screen_capture_command)}"),
                title="screen",
            )
        )

        return f"image:{filename}"

    # Provide any system instructions for the model
    # This can be generated dynamically, and is run at startup time
    def system(self) -> str:
        return """**When the user wants you to help debug, or work on a visual design by looking at their screen, IDE or browser, call the take_screenshot and send the output from the user.**"""  # noqa: E501
