import os
from datetime import datetime
from pathlib import Path
from typing import Optional

import click
from rich import print
from ruamel.yaml import YAML

from goose.cli.config import SESSIONS_PATH
from goose.cli.session import Session
from goose.toolkit.utils import render_template, parse_plan
from goose.utils import load_plugins
from goose.utils.autocomplete import SUPPORTED_SHELLS, setup_autocomplete
from goose.utils.session_file import list_sorted_session_files

LOG_LEVELS = ["DEBUG", "INFO", "WARNING", "ERROR", "CRITICAL"]
LOG_CHOICE = click.Choice(LOG_LEVELS)


@click.group()
def goose_cli() -> None:
    pass


@goose_cli.command(name="version")
def get_version() -> None:
    """Lists the version of goose and any plugins"""
    from importlib.metadata import entry_points, version

    print(f"[green]Goose-ai[/green]: [bold][cyan]{version('goose-ai')}[/cyan][/bold]")
    print("[green]Plugins[/green]:")
    entry_points = entry_points(group="metadata.plugins")
    modules = set()

    for ep in entry_points:
        module_name = ep.name
        modules.add(module_name)
    modules.remove("goose-ai")
    for module in sorted(list(modules)):
        # TODO: figure out how to get this to work for goose plugins block
        # as the module name is set to block.goose.cli
        # module_name = 'goose-plugins-block'
        try:
            module_version = version(module)
            print(f"  Module: [green]{module}[/green], Version: [bold][cyan]{module_version}[/cyan][/bold]")
        except Exception as e:
            print(f"  [red]Could not retrieve version for {module}: {e}[/red]")


def get_current_shell() -> str:
    return os.getenv("SHELL", "").split("/")[-1]


@goose_cli.command(name="shell-completions", help="Manage shell completions for goose")
@click.option("--install", is_flag=True, help="Install shell completions")
@click.option("--generate", is_flag=True, help="Generate shell completions")
@click.argument(
    "shell",
    type=click.Choice(SUPPORTED_SHELLS),
    default=get_current_shell(),
)
@click.pass_context
def shell_completions(ctx: click.Context, install: bool, generate: bool, shell: str) -> None:
    """Generate or install shell completions for goose

    Args:
        shell (str): shell to install completions for
        install (bool): installs completions if true, otherwise generates
                        completions
    """
    if not any([install, generate]):
        print("[red]One of --install or --generate must be specified[/red]\n")
        raise click.UsageError(ctx.get_help())

    if sum([install, generate]) > 1:
        print("[red]Only one of --install or --generate can be specified[/red]\n")
        raise click.UsageError(ctx.get_help())

    setup_autocomplete(shell, install=install)


@goose_cli.group()
def session() -> None:
    """Start or manage sessions"""
    pass


@goose_cli.group()
def toolkit() -> None:
    """Manage toolkits"""
    pass


@toolkit.command(name="list")
def list_toolkits() -> None:
    print("[green]Available toolkits:[/green]")
    for toolkit_name, toolkit in load_plugins("goose.toolkit").items():
        first_line_of_doc = toolkit.__doc__.split("\n")[0]
        print(f" - [bold]{toolkit_name}[/bold]: {first_line_of_doc}")


@goose_cli.group()
def providers() -> None:
    """Manage providers"""
    pass


@providers.command(name="list")
def list_providers() -> None:
    providers = load_plugins(group="exchange.provider")

    for provider_name, provider in providers.items():
        lines_doc = provider.__doc__.split("\n")
        first_line_of_doc = lines_doc[0]
        print(f" - [bold]{provider_name}[/bold]: {first_line_of_doc}")
        envs = provider.REQUIRED_ENV_VARS
        if envs:
            env_required_str = ", ".join(envs)
            print(f"        [dim]env vars required: {env_required_str}")

        print("\n")


def autocomplete_session_files(ctx: click.Context, args: str, incomplete: str) -> None:
    return [
        f"{session_name}"
        for session_name in sorted(get_session_files().keys(), reverse=True, key=lambda x: x.lower())
        if session_name.startswith(incomplete)
    ]


def get_session_files() -> dict[str, Path]:
    return list_sorted_session_files(SESSIONS_PATH)


@session.command(name="start")
@click.argument("name", required=False, shell_complete=autocomplete_session_files)
@click.option("--profile")
@click.option("--plan", type=click.Path(exists=True))
@click.option("--log-level", type=LOG_CHOICE, default="INFO")
def session_start(name: Optional[str], profile: str, log_level: str, plan: Optional[str] = None) -> None:
    """Start a new goose session"""
    if plan:
        yaml = YAML()
        with open(plan, "r") as f:
            _plan = yaml.load(f)
    else:
        _plan = None
    session = Session(name=name, profile=profile, plan=_plan, log_level=log_level)
    session.run()


def parse_args(ctx: click.Context, param: click.Parameter, value: str) -> dict[str, str]:
    if not value:
        return {}
    args = {}
    for item in value.split(","):
        key, val = item.split(":")
        args[key.strip()] = val.strip()

    return args


@session.command(name="planned")
@click.option("--plan", type=click.Path(exists=True))
@click.option("--log-level", type=LOG_CHOICE, default="INFO")
@click.option("-a", "--args", callback=parse_args, help="Args in the format arg1:value1,arg2:value2")
def session_planned(plan: str, log_level: str, args: Optional[dict[str, str]]) -> None:
    plan_templated = render_template(Path(plan), context=args)
    _plan = parse_plan(plan_templated)
    session = Session(plan=_plan, log_level=log_level)
    session.run()


@session.command(name="resume")
@click.argument("name", required=False, shell_complete=autocomplete_session_files)
@click.option("--profile")
@click.option("--log-level", type=LOG_CHOICE, default="INFO")
def session_resume(name: Optional[str], profile: str, log_level: str) -> None:
    """Resume an existing goose session"""
    session_files = get_session_files()
    if name is None:
        if session_files:
            name = list(session_files.keys())[0]
            print(f"Resuming most recent session: {name} from {session_files[name]}")
        else:
            print("No sessions found.")
            return
    else:
        if name in session_files:
            print(f"Resuming session: {name}")
        else:
            print(f"Creating new session: {name}")
    session = Session(name=name, profile=profile, log_level=log_level)
    session.run(new_session=False)


@goose_cli.command(name="run")
@click.argument("message_file", required=False, type=click.Path(exists=True))
@click.option("--profile")
@click.option("--log-level", type=LOG_CHOICE, default="INFO")
def run(message_file: Optional[str], profile: str, log_level: str) -> None:
    """Run a single-pass session with a message from a markdown input file"""
    if message_file:
        with open(message_file, "r") as f:
            initial_message = f.read()
    else:
        initial_message = click.get_text_stream("stdin").read()

    session = Session(profile=profile, log_level=log_level)
    session.single_pass(initial_message=initial_message)


@session.command(name="list")
def session_list() -> None:
    """List goose sessions"""
    session_files = get_session_files().items()
    for session_name, session_file in session_files:
        print(f"{datetime.fromtimestamp(session_file.stat().st_mtime).strftime('%Y-%m-%d %H:%M:%S')}    {session_name}")


@session.command(name="clear")
@click.option("--keep", default=3, help="Keep this many entries, default 3")
def session_clear(keep: int) -> None:
    """Delete old goose sessions, keeping the most recent sessions up to the specified number"""
    for i, (_, session_file) in enumerate(get_session_files().items()):
        if i >= keep:
            session_file.unlink()


@click.group(
    invoke_without_command=True,
    name="goose",
    help="AI-powered tool to assist in solving programming and operational tasks",
)
@click.option("-V", "--version", is_flag=True, help="List the version of goose and any plugins")
@click.pass_context
def cli(ctx: click.Context, version: bool, **kwargs: dict) -> None:
    if version:
        ctx.invoke(get_version)
        ctx.exit()
    elif ctx.invoked_subcommand is None:
        click.echo(ctx.get_help())


all_cli_group_options = load_plugins("goose.cli.group_option")
for option in all_cli_group_options.values():
    cli = option()(cli)

all_cli_groups = load_plugins("goose.cli.group")
for group in all_cli_groups.values():
    for command in group.commands.values():
        cli.add_command(command)

if __name__ == "__main__":
    cli()
