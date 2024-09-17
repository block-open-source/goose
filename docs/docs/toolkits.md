# Toolkits

This page contains information about building and using toolkits in Goose. Toolkits are a way to extend Goose's capabilities by adding new tools and functionalities. You can create your own toolkits or use the existing ones provided by Goose.

## Using Toolkits

Use `goose toolkit list` to list the available toolkits.

### Toolkits defined in Goose

Using Goose with toolkits is simple. You can add toolkits to your profile in the `profiles.yaml` file. Here's an example of how to add `my-toolkit` toolkit to your profile:

```yaml
my-profile:
  provider: openai
  processor: gpt-4o
  accelerator: gpt-4o-mini
  moderator: passive
  toolkits:
    - my-toolkit
```

Then run Goose with the specified profile:

```sh
goose session start --profile my-profile
```

### Toolkits defined in Goose Plugins

1. First make sure that `goose-plugins` is intalled with Goose:
```sh
pipx install goose-ai --preinstall goose-plugins
```
2. Update the `profiles.yaml` file to include the desired toolkit:
```yaml
my-profile:
  provider: openai
  processor: gpt-4o
  accelerator: gpt-4o-mini
  moderator: passive
  toolkits:
    - my-goose-plugins-toolkit
```

## Building a Toolkit

To add a toolkit, in your code (which doesn't necessarily need to be in the Goose package thanks to [plugin metadata][plugin]!), create a class that derives from the `Toolkit` class.

### Example toolkit class
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

### Exposing the new toolkit to Goose

To make the toolkit available, add it to the `pyproject.toml` file and then update your `profiles.yaml` file.

#### Update the `pyproject.toml` file
If you're adding the new toolkit to Goose or the Goose Plugins repo, simply find the `[project.entry-points."goose.toolkit"]` section in `pyproject.toml` and add a line like this:
```toml
[project.entry-points."goose.toolkit"]
developer = "goose.toolkit.developer:Developer"
github = "goose.toolkit.github:Github"
# Add a line like this - the key becomes the name used in profiles
demo = "goose.toolkit.demo:Demo"
```

If you are adding the toolkit to a different package, see the docs for `goose-plugins` for more information on how to create a plugins repository that can be used by Goose.

#### Update the `profiles.yaml` file
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

> [!NOTE]
> If you're using a plugin from `goose-plugins`, make sure `goose-plugins` is installed in your environment. You can install it via pip: 
> 
> `pipx install goose-ai --preinstall goose-plugins`

## Available Toolkits in Goose

To see the available toolkits to you, run `goose toolkit list`, this will show the toolkits defined below as well as any other Goose modules you have installed (for example, `goose-plugins`).

Goose provides a variety of toolkits designed to help developers with different tasks. Here's an overview of each available toolkit and its functionalities:

### 1. Developer Toolkit

The **Developer** toolkit offers general-purpose development capabilities, including:

- **System Configuration Details:** Retrieves system configuration details.
- **Task Management:** Update the plan by overwriting all current tasks.
- **File Operations:**
    - `patch_file`: Patch a file by replacing specific content.
    - `read_file`: Read the content of a specified file.
    - `write_file`: Write content to a specified file.
- **Shell Command Execution:** Execute shell commands with safety checks.

### 2. GitHub Toolkit

The **GitHub** toolkit provides detailed configuration and procedural guidelines for GitHub operations.

### 3. Lint Toolkit

The **Lint** toolkit ensures that all toolkits have proper documentation. It performs the following checks:

- Toolkit must have a docstring.
- The first line of the docstring should contain more than 5 words and fewer than 12 words.
- The first letter of the docstring should be capitalized.

### 4. RepoContext Toolkit

The **RepoContext** toolkit provides context about the current repository. It includes:

- **Repository Size:** Get the size of the repository.
- **Monorepo Check:** Determine if the repository is a monorepo.
- **Project Summarization:** Summarize the current project based on the repository or the current project directory.

### 5. Screen Toolkit

The **Screen** toolkit assists users in taking screenshots for debugging or designing purposes. It provides:

- **Take Screenshot:** Capture a screenshot and provide the path to the screenshot file.
- **System Instructions:** Instructions on how to work with screenshots.

### 6. SummarizeRepo Toolkit

The **SummarizeRepo** toolkit helps in summarizing a repository. It includes:

- **Summarize Repository:** Clone the repository (if not already cloned) and summarize the files based on specified extensions.

### 7. SummarizeProject Toolkit

The **SummarizeProject** toolkit generates or retrieves a summary of a project directory based on specified file extensions. It includes:

- **Get Project Summary:** Generate or retrieve a summary of the project in the specified directory.

### 8. SummarizeFile Toolkit

The **SummarizeFile** toolkit helps in summarizing a specific file. It includes:

- **Summarize File:** Summarize the contents of a specified file with optional instructions.

[plugin]: https://packaging.python.org/en/latest/guides/creating-and-discovering-plugins/#using-package-metadata
[goose-plugins]: https://github.com/square/goose-plugins
