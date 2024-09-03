from pathlib import Path
from typing import Final

GOOSE_GLOBAL_PATH: Final = Path("~/.config/goose").expanduser()
PROFILES_CONFIG_PATH: Final = GOOSE_GLOBAL_PATH.joinpath("profiles.yaml")
SESSIONS_PATH: Final = GOOSE_GLOBAL_PATH.joinpath("sessions")
SESSION_FILE_SUFFIX: Final = ".jsonl"
