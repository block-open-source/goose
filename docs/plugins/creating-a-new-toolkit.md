# Creating a New Toolkit

To add a toolkit, in your code (which doesn't necessarily need to be in the Goose package thanks to [plugin metadata][plugin]!), create a class that derives from the `Toolkit` class.

## Example toolkit class
Below is an example of a simple toolkit called `Demo` that derives from the `Toolkit` class. This toolkit provides an `authenticate` tool that outputs an authentication code for a user. It also provides system instructions for the model. 
```python
import os
import platform

from goose.toolkit.base import Toolkit, tool


class Demo(Toolkit):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)

    # Provide any additional tools as needed!
    # The docstring of the tool provides instructions to the LLM, so they are important to tune
    # you do not have to provide any tools, but any function decorated with @tool will be available
    @tool
    def authenticate(self, user: str):
        """Output an authentication code for this user

        Args:
            user (str): The username to authenticate for
        """
        # notifier supports any rich renderable https://rich.readthedocs.io/en/stable/introduction.html#quick-start
        self.notifier.log(f"[bold red]auth: {str(hash(user))}[/]")

    # Provide any system instructions for the model
    # This can be generated dynamically, and is run at startup time
    def system(self) -> str:
        print("new")
        return f"""**You must preceed your first message by using the authenticate tool for the current user**

        ```
        platform: {platform.system()}
        cwd: {os.getcwd()}
        user: {os.environ.get('USER')}
        ```
        """
```

## Exposing the New Toolkit to Goose

To make the toolkit available, add it to the `pyproject.toml` file and then update your `profiles.yaml` file.

### Update the `pyproject.toml` file
If you're adding the new toolkit to Goose or the Goose Plugins repo, simply find the `[project.entry-points."goose.toolkit"]` section in `pyproject.toml` and add a line like this:
```toml
[project.entry-points."goose.toolkit"]
developer = "goose.toolkit.developer:Developer"
github = "goose.toolkit.github:Github"
# Add a line like this - the key becomes the name used in profiles
demo = "goose.toolkit.demo:Demo"
```

If you are adding the toolkit to a different package, see the docs for `goose-plugins` for more information on how to create a plugins repository that can be used by Goose.

### Update the `profiles.yaml` file
And then to set up a profile that uses it, add something to ~/.config/goose/profiles.yaml
```yaml
default:
  provider: openai
  processor: gpt-4o
  accelerator: gpt-4o-mini
  moderator: passive
  toolkits:
    - name: developer
      requires: {}
demo-profile:
  provider: openai
  processor: gpt-4o
  accelerator: gpt-4o-mini
  moderator: passive
  toolkits:
    - developer
    - demo
```

And now you can run goose with this new profile to use the new toolkit!

```sh
goose session start --profile demo-profile
```

> [!NOTE]
> If you're using a plugin from `goose-plugins`, make sure `goose-plugins` is installed in your environment. You can install it via pip: 
> 
> `pipx install goose-ai --preinstall goose-plugins`

[plugin]: https://packaging.python.org/en/latest/guides/creating-and-discovering-plugins/#using-package-metadata
[goose-plugins]: https://github.com/block-open-source/goose-plugins
