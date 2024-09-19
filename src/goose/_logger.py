import logging
from pathlib import Path

_LOGGER_NAME = "goose"
_LOGGER_FILE_NAME = "goose.log"


def setup_logging(log_file_directory: Path, level: int = logging.INFO) -> None:
    logger = logging.getLogger(_LOGGER_NAME)
    logger.setLevel(level)
    log_file_directory.mkdir(parents=True, exist_ok=True)
    file_handler = logging.FileHandler(log_file_directory / _LOGGER_FILE_NAME)
    logger.addHandler(file_handler)
    formatter = logging.Formatter("%(asctime)s - %(name)s - %(levelname)s - %(message)s")
    file_handler.setFormatter(formatter)
    return logger


def get_logger() -> logging.Logger:
    return logging.getLogger(_LOGGER_NAME)
