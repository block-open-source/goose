import json
import sys
from datetime import datetime
from pathlib import Path
from typing import Dict, Optional

import click
from rich import print
from ruamel.yaml import YAML

from goose.cli.config import SESSIONS_PATH
from goose.cli.session import Session
from goose.utils import load_plugins
from goose.utils.session_file import list_sorted_session_files

def add_group_option(group, option):
    """
    Adds a given option to the group.
    
    :param group: The Click group to which the option will be added.
    :param option: The Click option to add.
    """
    group = option(group)
    return group

@click.group()
def goose_cli() -> None:
    pass


@goose_cli.command()
def version() -> None:
    """Lists the version of goose and any plugins"""
    from importlib.metadata import entry_points, version

    print(f"[green]Goose[/green]: [bold][cyan]{version('goose')}[/cyan][/bold]")
    print("[green]Plugins[/green]:")
    filtered_groups = {}
    modules = set()
    if sys.version_info.minor >= 12:
        for ep in entry_points():
            group = getattr(ep, "group", None)
            if group and (group.startswith("exchange.") or group.startswith("goose.")):
                filtered_groups.setdefault(group, []).append(ep)
        for eps in filtered_groups.values():
            for ep in eps:
                module_name = ep.module.split(".")[0]
                modules.add(module_name)
    else:
        eps = entry_points()
        for group, entries in eps.items():
            if group.startswith("exchange.") or group.startswith("goose."):
                for entry in entries:
                    module_name = entry.value.split(".")[0]
                    modules.add(module_name)

    modules.remove("goose")
    for module in sorted(list(modules)):
        # TODO: figure out how to get this to work for goose plugins block
        # as the module name is set to block.goose.cli
        # module_name = 'goose-plugins-block'
        try:
            module_version = version(module)
            print(f"  Module: [green]{module}[/green], Version: [bold][cyan]{module_version}[/cyan][/bold]")
        except Exception as e:
            print(f"  [red]Could not retrieve version for {module}: {e}[/red]")


@goose_cli.group()
def session() -> None:
    """Start or manage sessions"""
    pass


@session.command(name="start")
@click.option("--profile")
@click.option("--plan", type=click.Path(exists=True))
def session_start(profile: str, plan: Optional[str] = None) -> None:
    """Start a new goose session"""
    if plan:
        yaml = YAML()
        with open(plan, "r") as f:
            _plan = yaml.load(f)
    else:
        _plan = None

    session = Session(profile=profile, plan=_plan)
    session.run()


@session.command(name="resume")
@click.argument("name", required=False)
@click.option("--profile")
def session_resume(name: str, profile: str) -> None:
    """Resume an existing goose session"""
    if name is None:
        session_files = get_session_files()
        if session_files:
            name = list(session_files.keys())[0]
            print(f"Resuming most recent session: {name} from {session_files[name]}")
        else:
            print("No sessions found.")
            return
    session = Session(name=name, profile=profile)
    session.run()


@session.command(name="list")
def session_list() -> None:
    session_files = get_session_files().items()
    for session_name, session_file in session_files:
        print(f"{datetime.fromtimestamp(session_file.stat().st_mtime).strftime('%Y-%m-%d %H:%M:%S')}    {session_name}")


@session.command(name="clear")
@click.option("--keep", default=3, help="Keep this many entries, default 3")
def session_clear(keep: int) -> None:
    for i, (_, session_file) in enumerate(get_session_files().items()):
        if i >= keep:
            session_file.unlink()


def get_session_files() -> Dict[str, Path]:
    return list_sorted_session_files(SESSIONS_PATH)


def describe_command(cmd: click.Command) -> dict:
    ret = {
        "name": cmd.name,
        "summary": cmd.get_short_help_str().rstrip("."),
    }

    if isinstance(cmd, click.Group):
        ret["commands"] = [
            describe_command(subcmd) for name, subcmd in cmd.commands.items()
        ]

    return ret


def describe_callback(ctx, param, value):
    if not value or ctx.resilient_parsing:
        return

    desc = describe_command(ctx.command)
    desc["name"] = ctx.info_name  # Set the correct name
    desc["summary"] = ctx.command.get_short_help_str().rstrip(".")  # Set the summary
    click.echo(json.dumps(desc, sort_keys=True, indent=2))
    ctx.exit()


describe_option = click.option(
    "--describe-commands",
    is_flag=True,
    type=bool,
    help="List command descriptions for sq integration and exit.",
    hidden=True,
    default=False,
    is_eager=True,
    callback=describe_callback,
)


@click.group(invoke_without_command=True, name="tea",
    help="CLI tool to retrieve Temporary Elevated Access (TEA) credentials",)
# @describe_option
@click.pass_context
def cli(ctx, describe_commands):
    pass

cli = add_group_option(cli, describe_option)
all_clis = load_plugins("goose.cli")
for group in all_clis.values():
    for command in group.commands.values():
        cli.add_command(command)


if __name__ == "__main__":
    # merge_commands()
    cli()
