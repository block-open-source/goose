from functools import cache
from io import StringIO
from typing import Callable, Mapping, Dict, Optional, Tuple

from rich import print
from rich.panel import Panel
from rich.prompt import Confirm
from rich.text import Text
from ruamel.yaml import YAML

from ...config import PROFILES_CONFIG_PATH
from ...profile import Profile
from ...utils import load_plugins
from .diff import pretty_diff


@cache
def _all_recommended_profiles() -> Mapping[str, Callable]:
    return load_plugins(group="goose.profile")


def _write_config(profiles: Dict[str, Profile]) -> None:
    """Overwrite the config with the passed profiles"""
    PROFILES_CONFIG_PATH.parent.mkdir(parents=True, exist_ok=True)
    converted = {name: profile.to_dict() for name, profile in profiles.items()}
    yaml = YAML()
    with PROFILES_CONFIG_PATH.open("w") as f:
        yaml.dump(converted, f)


def _ensure_config(name: str) -> Profile:
    """Ensure that the config exists and has the default section"""
    # TODO we should copy a templated default config in to better document
    # but this is complicated a bit by autodetecting the provider

    provider, processor, accelerator = _default_model_configuration()
    profile = _all_recommended_profiles()[name](provider, processor, accelerator)

    profiles = {}
    if not PROFILES_CONFIG_PATH.exists():
        print(
            Panel(
                f"[yellow]No configuration present, we will create a profile '{name}'"
                + f" at: [/]{str(PROFILES_CONFIG_PATH)}\n"
                + "You can add your own profile in this file to further configure goose!"
            )
        )
        default = profile
        profiles = {name: default}
        _write_config(profiles)
        return profile

    profiles = _read_config()
    if name not in profiles:
        print(Panel(f"[yellow]Your configuration doesn't have a profile named '{name}', adding one now[/yellow]"))
        profiles.update({name: profile})
        _write_config(profiles)
    elif name in profiles:
        # if the profile stored differs from the default one, we should prompt the user to see if they want
        # to update it! we need to recursively compare the two profiles, as object comparison will always return false
        is_profile_eq = profile.to_dict() == profiles[name].to_dict()
        if not is_profile_eq:
            yaml = YAML()
            before = StringIO()
            after = StringIO()
            yaml.dump(profiles[name].to_dict(), before)
            yaml.dump(profile.to_dict(), after)
            before.seek(0)
            after.seek(0)

            print(
                Panel(
                    Text(
                        f"Your profile uses one of the default options - '{name}'"
                        + " - but it differs from the latest version:\n\n",
                    )
                    + pretty_diff(before.read(), after.read())
                )
            )
            should_update = Confirm.ask(
                "Do you want to update your profile to use the latest?",
                default=False,
            )
            if should_update:
                profiles[name] = profile
                _write_config(profiles)
            else:
                profile = profiles[name]

    return profile


def _read_config() -> Dict[str, Profile]:
    """Return config from the configuration file and validates its contents"""

    yaml = YAML()
    with PROFILES_CONFIG_PATH.open("r") as f:
        data = yaml.load(f)

    return {name: Profile(**profile) for name, profile in data.items()}


def _default_model_configuration() -> Tuple[str, str, str]:
    providers = load_plugins(group="exchange.provider")
    for provider, cls in providers.items():
        try:
            cls.from_env()
            print(Panel(f"[green]Detected an available provider: [/]{provider}"))
            break
        except Exception:
            pass
    else:
        raise ValueError(
            "Could not detect an available provider,"
            + " make sure to plugin a provider or set an env var such as OPENAI_API_KEY"
        )

    recommended = {
        "openai": ("gpt-4o", "gpt-4o-mini"),
        "anthropic": (
            "claude-3-5-sonnet-20240620",
            "claude-3-5-sonnet-20240620",
        ),
        "databricks": (
            # TODO when function calling is first rec should be: "databricks-meta-llama-3-1-405b-instruct"
            "databricks-meta-llama-3-1-70b-instruct",
            "databricks-meta-llama-3-1-70b-instruct",
        ),
    }
    processor, accelerator = recommended.get(provider, ("gpt-4o", "gpt-4o-mini"))
    return provider, processor, accelerator


def load_profile(name: Optional[str]) -> Profile:
    if name is None:
        name = "default"

    # If the name is one of the default values, we ensure a valid configuration
    if name in _all_recommended_profiles():
        return _ensure_config(name)

    # Otherwise this is a custom config and we return it from the config file
    return _read_config()[name]
