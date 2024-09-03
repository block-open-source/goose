from pathlib import Path

GOOSE_GLOBAL_PATH = Path("~/.config/goose").expanduser()
PROFILES_CONFIG_PATH = GOOSE_GLOBAL_PATH.joinpath("profiles.yaml")
SESSIONS_PATH = GOOSE_GLOBAL_PATH.joinpath("sessions")
SESSION_FILE_SUFFIX = ".jsonl"
