# Configuring Goose

## Tuning it to your repo

Goose ships with the ability to read in the contents of a file named `.goosehints` from your repo. If you find yourself repeating the same information across sessions to Goose, this file is the right place to add this information.

> [!NOTE] 
> `.goosehints` follows [jinja templating rules][jinja-guide] in case you want to take advantage of this.

## Profiles

If you need to customize goose, one way is via editing: `~/.config/goose/profiles.yaml`.

It will look by default something like:

```yaml
default:
  provider: block
  processor: gpt-4o
  accelerator: gpt-4o-mini
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
  provider: block
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
