# Configuring Goose

## Profiles

If you need to customize goose, one way is via editing: `~/.config/goose/profiles.yaml`.

It will look by default something like (and when you run `goose session start` without the `--profile` flag it will use the `default` profile):

```yaml
default:
  provider: open-ai
  processor: gpt-4o
  accelerator: gpt-4o-mini
  moderator: passive
  toolkits:
    - name: developer
      requires: {}
```

### Fields 

#### provider

Provider of LLM. LLM providers that currently are supported by Goose:

| Provider | Required environment variable(s) to access provider |
| ----- | ------------------------------ |
| openai | `OPENAI_API_KEY` |
| anthropic  | `ANTHROPIC_API_KEY` |
| databricks | `DATABRICKS_HOST` and `DATABRICKS_TOKEN` |

#### processor

This is the model used for the main Goose loop and main tools -- it should be be capable of complex, multi-step tasks such as writing code and executing commands. Example: `gpt-4o`. You should choose the model based the provider you configured.

#### accelerator

Small model for fast, lightweight tasks. Example: `gpt-4o-mini`. You should choose the model based the provider you configured.

#### moderator

Rules designed to control or manage the output of the model. Moderators that currently are supported by Goose:

- `passive`: does not actively intervene in every response
- `truncate`: truncates the first contexts when the contexts exceed the max token size

### Example `profiles.yaml` files

#### provider as `anthropic`

```yaml

default:
  provider: anthropic
  processor: claude-3-5-sonnet-20240620
  accelerator: claude-3-5-sonnet-20240620
```

#### provider as `databricks`

```yaml
default:
  provider: databricks
  processor: databricks-meta-llama-3-1-70b-instruct
  accelerator: databricks-meta-llama-3-1-70b-instruct
  moderator: passive
  toolkits:
    - name: developer
      requires: {}
```

You can tell it to use another provider for example for Anthropic:

```yaml
default:
  provider: anthropic
  processor: claude-3-5-sonnet-20240620
  accelerator: claude-3-5-sonnet-20240620
  moderator: passive
  toolkits:
    - name: developer
      requires: {}
```

this will then use the claude-sonnet model, you will need to set the `ANTHROPIC_API_KEY` to your anthropic API key.

You can also customize Goose's behavior through toolkits. These are set up automatically for you in the same `~/.config/goose/profiles.yaml` file, but you can include or remove toolkits as you see fit.

For example, Goose's `unit-test-gen` command sets up a new profile in this file for you:

```yaml
unit-test-gen:
  provider: openai
  processor: gpt-4o
  accelerator: gpt-4o-mini
  moderator: passive
  toolkits:
    - name: developer
      requires: {}
    - name: unit-test-gen
      requires: {}
    - name: java
      requires: {}
```

[jinja-guide]: https://jinja.palletsprojects.com/en/3.1.x/


## Adding a toolkit
To make a toolkit available to Goose, add it to your project's pyproject.toml. For example in the Goose pyproject.toml file:
```
[project.entry-points."goose.toolkit"]
developer = "goose.toolkit.developer:Developer"
github = "goose.toolkit.github:Github"
# Add a line like this - the key becomes the name used in profiles
my-new-toolkit = "goose.toolkit.my_toolkits:MyNewToolkit"  # this is the path to the class that implements the toolkit
```

Then to set up a profile that uses it, add something to `~/.config/goose/profiles.yaml`:
```yaml
my-profile:
  provider: openai
  processor: gpt-4o
  accelerator: gpt-4o-mini
  moderator: passive
  toolkits:  # new toolkit gets added here
    - developer
    - my-new-toolkit
```

And now you can run Goose with this new profile to use the new toolkit!

```sh
goose session start --profile my-profile
```

Or, if you're developing a new toolkit and want to test it:
```sh
uv run goose session start --profile my-profile
```

## Tuning Goose to your repo

Goose ships with the ability to read in the contents of a file named `.goosehints` from your repo. If you find yourself repeating the same information across sessions to Goose, this file is the right place to add this information.

This file will be read into the Goose system prompt if it is present in the current working directory.

> [!NOTE] 
> `.goosehints` follows [jinja templating rules][jinja-guide] in case you want to leverage templating to insert file contents or variables.