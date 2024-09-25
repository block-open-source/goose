import sys
from pathlib import Path

from rich import print

SUPPORTED_SHELLS = ["bash", "zsh", "fish"]


def is_autocomplete_installed(file: Path) -> bool:
    if not file.exists():
        print(f"[yellow]{file} does not exist, creating file")
        with open(file, "w") as f:
            f.write("")

    # https://click.palletsprojects.com/en/8.1.x/shell-completion/#enabling-completion
    if "_GOOSE_COMPLETE" in open(file).read():
        print(f"auto-completion already installed in {file}")
        return True
    return False


def setup_bash(install: bool) -> None:
    bashrc = Path("~/.bashrc").expanduser()
    if install:
        if is_autocomplete_installed(bashrc):
            return
        f = open(bashrc, "a")
    else:
        f = sys.stdout
        print(f"# add the following to your bash config, typically {bashrc}")

    with f:
        f.write('eval "$(_GOOSE_COMPLETE=bash_source goose)"\n')

    if install:
        print(f"installed auto-completion to {bashrc}")
        print(f"run `source {bashrc}` to enable auto-completion")


def setup_fish(install: bool) -> None:
    completion_dir = Path("~/.config/fish/completions").expanduser()
    if not completion_dir.exists():
        completion_dir.mkdir(parents=True, exist_ok=True)

    completion_file = completion_dir / "goose.fish"
    if install:
        if is_autocomplete_installed(completion_file):
            return
        f = open(completion_file, "a")
    else:
        f = sys.stdout
        print(f"# add the following to your fish config, typically {completion_file}")

    with f:
        f.write("_GOOSE_COMPLETE=fish_source goose | source\n")

    if install:
        print(f"installed auto-completion to {completion_file}")


def setup_zsh(install: bool) -> None:
    zshrc = Path("~/.zshrc").expanduser()
    if install:
        if is_autocomplete_installed(zshrc):
            return
        f = open(zshrc, "a")
    else:
        f = sys.stdout
        print(f"# add the following to your zsh config, typically {zshrc}")

    with f:
        f.write("autoload -U +X compinit && compinit\n")
        f.write("autoload -U +X bashcompinit && bashcompinit\n")
        f.write('eval "$(_GOOSE_COMPLETE=zsh_source goose)"\n')

    if install:
        print(f"installed auto-completion to {zshrc}")
        print(f"run `source {zshrc}` to enable auto-completion")


def setup_autocomplete(shell: str, install: bool) -> None:
    """Installs shell completions for goose

    Args:
        shell (str): shell to install completions for
        install (bool): whether to install or generate completions
    """

    match shell:
        case "bash":
            setup_bash(install=install)

        case "zsh":
            setup_zsh(install=install)

        case "fish":
            setup_fish(install=install)

        case _:
            print(f"Shell {shell} not supported")
