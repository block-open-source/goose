# Using Toolkits

Use `goose toolkit list` to list the available toolkits.

## Toolkits defined in Goose

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

## Toolkits defined in Goose Plugins

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

