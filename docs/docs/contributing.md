# Contributing

- [Guide](#guide)
- [Building from source](#building-from-source)

## Guide

Goose is highly configurable through plugins - it reads in modules that its dependencies install (e.g.`goose-plugins-block`) and uses those that start with certain prefixes (e.g. `goose.toolkit`) to inject their functionality. For example, you will note that Goose's CLI is actually merged with additional CLI methods that are exported from `goose-plugins-block`.

If you are building a net new feature, you should try to fit it inside a plugin.

> [!INFO]
> If you are making changes to the `pyproject.toml` file in `goose-plugins-block` (e.g. to add a new plugin), you must run `poetry sync` in `goose-plugins-block` to resync the plugins it exports.

## Building from source

If you want to develop features on `goose` and `goose-plugins-block` you can follow these steps:

1. Run `compost` with the `data` role:
```bash
~/Development/topsoil green ./compost -r data
```
2. Clone the `goose` and `goose-plugins-block` repos:
 ```bash
 git clone git@github.com:square/goose.git ~/Development/goose
 git clone org-49461806@github.com:squareup/goose-plugins-block.git ~/Development/goose-plugins-block
 ```
3. Get `uv` with `brew install uv`
4. Set up your `uv` config file at `~/.config/uv/config.toml`:
```toml
native-tls = true
index-url = "https://artifactory.global.square/artifactory/api/pypi/block-pypi/simple"
```
5. Set up your Python virtualenv:
```bash
cd ~/Development/goose
uv sync
uv venv
```
6. Install `goose-plugins-block` in `goose`. This means any changes to `goose-plugins-block` in this folder will immediately be reflected in `goose`:
```bash
uv add --editable ~/Development/goose-plugins-block
```
7. Run `goose`
```bash
uv run goose session start  # or any of goose's commands (e.g. goose --help)
```