import os
import sys
from typing import Optional
import keyring

from prompt_toolkit import prompt
from prompt_toolkit.shortcuts import confirm
from rich import print
from rich.panel import Panel

from goose.build import build_exchange
from goose.cli.config import PROFILES_CONFIG_PATH
from goose.cli.session_notifier import SessionNotifier
from goose.profile import Profile
from exchange import Exchange
from exchange.invalid_choice_error import InvalidChoiceError
from exchange.providers.base import MissingProviderEnvVariableError


def create_exchange(profile: Profile, notifier: SessionNotifier) -> Exchange:
    try:
        return build_exchange(profile, notifier=notifier)
    except InvalidChoiceError as e:
        error_message = (
            f"[bold red]{e.message}[/bold red].\nPlease check your configuration file at {PROFILES_CONFIG_PATH}.\n"
            + "Configuration doc: https://block-open-source.github.io/goose/configuration.html"
        )
        print(error_message)
        sys.exit(1)
    except MissingProviderEnvVariableError as e:
        api_key = _get_api_key_from_keychain(e.env_variable, e.provider)
        if api_key is None or api_key == "":
            error_message = f"{e.message}. Please set the required environment variable to continue."
            print(Panel(error_message, style="red"))
            sys.exit(1)
        else:
            os.environ[e.env_variable] = api_key
            return build_exchange(profile=profile, notifier=notifier)


def _get_api_key_from_keychain(env_variable: str, provider: str) -> Optional[str]:
    api_key = keyring.get_password("goose", env_variable)
    if api_key is not None:
        print(f"Using {env_variable} value for {provider} from your keychain")
    else:
        api_key = prompt(f"Enter {env_variable} value for {provider}:".strip())
        if api_key is not None and len(api_key) > 0:
            save_to_keyring = confirm(f"Would you like to save the {env_variable} value to your keychain?")
            if save_to_keyring:
                keyring.set_password("goose", env_variable, api_key)
                print(f"Saved {env_variable} to your key_chain. service_name: goose, user_name: {env_variable}")
    return api_key
