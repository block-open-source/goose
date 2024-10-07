import os
import logging
from typing import Callable
from dotenv import load_dotenv
import subprocess
import time
import platform
import socket
from langfuse.decorators import langfuse_context

logger = logging.getLogger(__name__)
logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)

HAS_LANGFUSE_CREDENTIALS = False
SCRIPT_DIR = os.path.dirname(os.path.realpath(__file__))
LANGFUSE_REPO_URL = "https://github.com/langfuse/langfuse.git"
LANGFUSE_CLONE_DIR = os.path.join(SCRIPT_DIR, "langfuse_clone")
LANGFUSE_LOCAL_ENV_FILE_NAME = ".env.langfuse.local"
LANGFUSE_LOCAL_INIT_ENV_FILE = os.path.join(SCRIPT_DIR, LANGFUSE_LOCAL_ENV_FILE_NAME)


def _run_command(command):
    """Run a shell command."""
    result = subprocess.run(command, shell=True, check=True, text=True)
    return result


def _update_or_clone_repo():
    """Update or clone the Langfuse repository."""
    if os.path.isdir(os.path.join(LANGFUSE_CLONE_DIR, ".git")):
        _run_command(f"git -C {LANGFUSE_CLONE_DIR} pull --rebase")
    else:
        _run_command(f"git clone {LANGFUSE_REPO_URL} {LANGFUSE_CLONE_DIR}")


def _copy_env_file():
    """Copy the environment file to the Langfuse clone directory."""
    if os.path.isfile(LANGFUSE_LOCAL_INIT_ENV_FILE):
        subprocess.run(["cp", LANGFUSE_LOCAL_INIT_ENV_FILE, LANGFUSE_CLONE_DIR], check=True)
    else:
        logger.error("Environment file not found. Exiting.")
        exit(1)


def _start_docker_compose():
    """Start Docker containers for Langfuse service and Postgres db."""
    _run_command(f"cd {LANGFUSE_CLONE_DIR} && docker compose --env-file ./{LANGFUSE_LOCAL_ENV_FILE_NAME} up --detach")


def _wait_for_service():
    """Wait for the Langfuse service to be available on localhost:3000."""
    while True:
        try:
            with socket.create_connection(("localhost", 3000), timeout=1):
                break
        except OSError:
            time.sleep(1)


def _launch_browser():
    """Launch the default web browser to open the Langfuse service URL."""
    system = platform.system().lower()
    url = "http://localhost:3000"

    if "linux" in system:
        subprocess.run(["xdg-open", url])
    elif "darwin" in system:  # macOS
        subprocess.run(["open", url])
    else:
        logger.info("Please open http://localhost:3000 to view Langfuse traces.")


def _print_login_variables():
    """Read and print the default email and password from the environment file."""
    if os.path.isfile(LANGFUSE_LOCAL_INIT_ENV_FILE):
        print("Please log in with the initialization environment variables:")
        env_vars = {}
        with open(LANGFUSE_LOCAL_INIT_ENV_FILE) as f:
            for line in f:
                if "=" in line:
                    key, value = line.strip().split("=", 1)
                    env_vars[key] = value
        print(f"Email: {env_vars.get('LANGFUSE_INIT_USER_EMAIL', 'Not found')}")
        print(f"Password: {env_vars.get('LANGFUSE_INIT_USER_PASSWORD', 'Not found')}")
    else:
        logger.warning("Langfuse environment file with local credentials required for local login not found.")


def setup_langfuse():
    """Main function to set up and run the Langfuse service."""
    try:
        _update_or_clone_repo()
        _copy_env_file()
        _start_docker_compose()
        _wait_for_service()
        _launch_browser()
        _print_login_variables()
        load_dotenv(LANGFUSE_LOCAL_INIT_ENV_FILE)
        if langfuse_context.auth_check():
            logger.info(f"Langfuse credentials found. Find traces, if enabled, under {os.environ['LANGFUSE_HOST']}.")
            global HAS_LANGFUSE_CREDENTIALS
            HAS_LANGFUSE_CREDENTIALS = True
    except Exception as e:
        logger.warning(f"Trouble finding Langfuse or Langfuse credentials: {e}")


def observe_wrapper(*args, **kwargs) -> Callable:
    """
    A decorator that wraps a function with Langfuse context observation if credentials are available.

    If Langfuse credentials are found, the function will be wrapped with Langfuse's observe method.
    Otherwise, the function will be returned as-is.

    Args:
        *args: Positional arguments to pass to langfuse_context.observe.
        **kwargs: Keyword arguments to pass to langfuse_context.observe.

    Returns:
        Callable: The wrapped function if credentials are available, otherwise the original function.
    """

    def _wrapper(fn: Callable) -> Callable:
        if HAS_LANGFUSE_CREDENTIALS:
            return langfuse_context.observe(*args, **kwargs)(fn)
        else:
            return fn

    return _wrapper

setup_langfuse()