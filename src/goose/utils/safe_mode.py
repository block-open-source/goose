import os


GOOSE_SAFE_MODE_ENV = "GOOSE_SAFE_MODE"

def set_safe_mode() -> None:
    os.environ[GOOSE_SAFE_MODE_ENV] = "true"
    return None

def is_in_safe_mode() -> bool:
    return os.getenv(GOOSE_SAFE_MODE_ENV, "false") == "true"

