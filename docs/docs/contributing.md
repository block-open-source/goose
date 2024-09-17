# Contributing

- [Guide](#guide)
- [Building from source](#building-from-source)

## Guide

Goose is highly configurable through plugins - it reads in modules that its dependencies install (e.g.`goose-plugins`) and uses those that start with certain prefixes (e.g. `goose.toolkit`) to inject their functionality. For example, you will note that Goose's CLI is actually merged with additional CLI methods that are exported from `goose-plugins-block`.

If you are building a net new feature, you should try to fit it inside a plugin.

> [!INFO]
> If you are making changes to the `pyproject.toml` file in `goose-plugins-block` (e.g. to add a new plugin), you must run `poetry sync` in `goose-plugins-block` to resync the plugins it exports.

## Building from source

If you want to develop features on `goose` and `goose-plugins` you can follow these steps:

1. Run `compost` with the `data` role:
```bash
~/Development/topsoil green ./compost -r data
```
2. Clone the `goose` and `goose-plugins-block` repos:
 ```bash
 git clone git@github.com:square/goose.git ~/Development/goose
 git clone git@github.com:square/goose-plugins.git ~/Development/goose-plugins
 ```
3. Get `uv` with `brew install uv`
4. Set up your Python virtualenv:
```bash
cd ~/Development/goose
uv sync
uv venv
```
5. Run the `source` command that follows the `uv venv` command to activate the virtualenv.
6. Install `goose-plugins` in `goose`. This means any changes to `goose-plugins` in this folder will immediately be reflected in `goose`:
```bash
uv add --editable ~/Development/goose-plugins
```
7. Run `goose`
```bash
uv run goose session start  # or any of goose's commands (e.g. goose --help)
```