# Installation

There are 2 ways to install goose at Block `sq` toolchain and using pipx. The `sq` toolchain is the preferred method as it is the best way to install goose and keep it up to date. The pipx method is useful if you want to develop goose  or its plugins. If you want to test the open-source version of goose with Block-specific plugins, you can use this method.

## 1. Through the `sq` toolchain

***Preferred method*: This is the best way to install goose and keep it up to date.**

Goose is available via the `sq` CLI. In order to get access to it, run the following commands:

```bash
sq update
sq packs add goose
```

The command `goose` is now accessible via `sq goose`. To update goose, simply run `sq get goose`.

## 2. Using pipx

> [!INFO]
> This installation method is under active development and may not be stable. *Last updated: 2024-09-05*

> [!NOTE]
> This is really only good for testing the latest public version of goose and Block-specific plugins.
> For further information about contributing to goose as a Block employee or developing plugins, please see 
> [Contributing](contributing.md).

First make sure you've [installed pipx][pipx] - for example:

```sh
brew install pipx
pipx ensurepath
```

Ensure you are on the warp “zero trust” VPN

Run following to setup block artifactory:
```sh
python3 -m pip config set global.index-url https://artifactory.global.square/artifactory/api/pypi/block-pypi/simple
python3 -m pip config set global.cert /opt/homebrew/etc/openssl@1.1/cert.pem
pipx ensurepath
```

make sure ~/.local/bin is on your path (edit ~/.zshrc if needed) and open a new terminal window.

> [!NOTE] 
> If you get the following error (this is likely to occur if you are using the data role from Square Compost):
> ```sh
> ERROR: Fatal Internal error [id=2]. Please report as a bug.
> ```
> You can try the following:
> `unset PIP_CONFIG_FILE` or comment this line `# export PIP_CONFIG_FILE=~/.config/pip/pip.conf`
> in `~/.zshrc`


Then you can install goose with

```sh
pipx install goose-ai --preinstall goose-plugins-block
```

To update it (as it is evolving rapidly):
```sh
pipx uninstall goose-ai
pipx install goose-ai --preinstall goose-plugins-block
```

You should be able to start using goose. Try out `goose version` as a test command.

If the above does not work, try: 
```sh
pipx install goose-plugins-block --include-deps
```

> [!NOTE]
> You may have to add `--python /path/to/python3.12` if python version in use is <3.12.

[pipx]: https://github.com/pypa/pipx?tab=readme-ov-file#install-pipx
