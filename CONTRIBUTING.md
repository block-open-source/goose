# Contributing

We welcome Pull Requests for general contributions. If you have a larger new feature or any questions on how
to develop a fix, we recommend you open an issue before starting.

## Prerequisites

We provide a shortcut to standard commands using [just][just] in our `justfile`.

* *goose* uses [uv][uv] for dependency management, and formats with [ruff][ruff] - install UV first: https://pypi.org/project/uv/ 

## Developing and testing

Now that you have a local environment, you can make edits and run our tests. 

```sh
uv run pytest tests -m "not integration"
```

or, as a shortcut, 

```sh
just test
```

## Running goose from source

`uv run goose session start`

will run a fresh goose session (can use the usual goose commands with `uv run` prefixed)

## Running ai-exchange from source

goose depends heavily on the https://github.com/square/exchange project, you can clone that repo and then work on both by running: 

```sh
uv add --editable <path/to/cloned/exchange>

then when you run goose with `uv run goose` it will be running it all from source. 

## Evaluations

Given that so much of *goose* involves interactions with LLMs, our unit tests only go so far to
confirming things work as intended.

We're currently developing a suite of evalutions, to make it easier to make improvements to *goose* more confidently.

In the meantime, we typically incubate any new additions that change the behavior of the *goose*
through **opt-in** plugins - `Toolkit`s, `Moderator`s, and `Provider`s. We welcome contributions of plugins 
that add new capabilities to *goose*. We recommend sending in several examples of the new capabilities 
in action with your pull request.

Additions to the [developer toolkit][developer] change the core performance, and so will need to be measured carefully.

## Build a Toolkit

To add a toolkit, start out with a plugin as mentioned above. In your code (which doesn't necessarily need to be
in the goose package thanks to [plugin metadata][plugin]!), create a class that derives from Toolkit.

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

To make the toolkit available, add it as a plugin. For example in a pyproject.toml
```
[project.entry-points."goose.toolkit"]
developer = "goose.toolkit.developer:Developer"
github = "goose.toolkit.github:Github"
# Add a line like this - the key becomes the name used in profiles
demo = "goose.toolkit.demo:Demo"
```

And then to setup a profile that uses it, add something to ~/.config/goose/profiles.yaml
```yaml
default:
  provider: openai
  processor: gpt-4o
  accelerator: gpt-4o-mini
  moderator: passive
  toolkits:
    - name: developer
      requires: {}
demo:
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
goose session start --profile demo
```

## Conventional Commits

This project follows the [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/) specification for PR titles. Conventional Commits make it easier to understand the history of a project and facilitate automation around versioning and changelog generation.

[developer]: src/goose/toolkit/developer.py
[uv]: https://docs.astral.sh/uv/
[ruff]: https://docs.astral.sh/ruff/
[just]: https://github.com/casey/just
[plugin]: https://packaging.python.org/en/latest/guides/creating-and-discovering-plugins/#using-package-metadata
