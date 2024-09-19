import os
import re
import subprocess
import uuid

from rich.markdown import Markdown
from rich.panel import Panel

from goose.toolkit.base import Toolkit, tool


class Screen(Toolkit):
    """Provides an instructions on when and how to work with screenshots"""

    @tool
    def take_screenshot(self, display: int = 1, resize: bool = False, target_size: int = 4) -> str:
        """
        Take a screenshot to assist the user in debugging or designing an app. They may need help with moving a screen element, or interacting in some way where you could do with seeing the screen.
        Optionally, resize the image using the `sips` cli tool. You may do this fs there is potential for error in the payload size.

        Args:
            display (int): Display to take the screen shot in. Default is the main display (1). Must be a value greater than 1.
            resize (bool): Boolean parameter to resize the image or not. No resizing by default.
            target_size (int): Target file size in MB. Default is 4.
        Return:
            (str) a path to the screenshot file, in the format of image: followed by the path to the file.
        """  # noqa: E501
        # Generate a random tmp filename for screenshot
        filename = f"/tmp/goose_screenshot_{uuid.uuid4().hex}.png"
        screen_capture_command = ["screencapture", "-x", "-D", str(display), filename]
        resize_command_str = ""

        subprocess.run(screen_capture_command)

        if resize:
            # get current disk size and reduce pixels by fractional amount to target (not linear but approx)
            size = os.path.getsize(filename) / (1024**2)
            reduce_by = max(0, (size - target_size) / size)
            output = subprocess.run(["sips", "-g", "pixelWidth", filename], stdout=subprocess.PIPE).stdout.decode()
            current_pixel_width = int(re.search(r"pixelWidth:\s*(\d+)", output).group(1))

            resize_command = ["sips", "--resampleWidth", str(int(current_pixel_width * reduce_by)), filename]
            subprocess.run(resize_command)
            resize_command_str = " ".join(resize_command)

        self.notifier.log(
            Panel.fit(
                Markdown(f"```bash\n{' '.join(screen_capture_command)}\n{resize_command_str if resize else ''}"),
                title="screen",
            )
        )

        return f"image:{filename}"

    # Provide any system instructions for the model
    # This can be generated dynamically, and is run at startup time
    def system(self) -> str:
        return """**When the user wants you to help debug, or work on a visual design by looking at their screen, IDE or browser, call the take_screenshot and send the output from the user.**"""  # noqa: E501
