import logging
from pathlib import Path
import json

_LOGGER_NAME = "goose"
_LOGGER_FILE_NAME = "goose.log"

_TRACE_LOGGER_NAME = "trace_logger"
_TRACE_LOGGER_FILE_NAME = "trace.log"

TRACE_LEVEL = 5
logging.addLevelName(TRACE_LEVEL, "TRACE")


def trace(self: logging.Logger, message: str, *args: tuple, **kws: dict) -> None:
    """
    Log 'message' with severity 'TRACE' to log agent traces.

    Usage: logger.trace("message")
    """
    if isinstance(message, dict):
        try:
            role = message.get("role", "")
            content_type = message.get("content", [{}])[0].get("type", "")
            if content_type == "ToolUse":
                tool = message.get("content", [{}])[0].get("name", "")
                content_type = f"{content_type} [{tool}]"
            content = message.get("content", [{}])[0]
            formatted_content = "\n" + json.dumps(content, indent=4)
            formatted_content = formatted_content.replace("\\n", "\n")
            formatted_message = f"** {role} / {content_type} ** \n{formatted_content}\n\n"
            message = formatted_message
        except Exception:
            message = "\n" + json.dumps(message, indent=4)
            message = message.replace("\\n", "\n")
    if self.isEnabledFor(TRACE_LEVEL):
        self._log(TRACE_LEVEL, message, args, **kws)


logging.Logger.trace = trace


def setup_logging(log_file_directory: Path, log_level: str = "INFO") -> None:
    logger = logging.getLogger(_LOGGER_NAME)
    logger.setLevel(getattr(logging, log_level))
    log_file_directory.mkdir(parents=True, exist_ok=True)
    file_handler = logging.FileHandler(log_file_directory / _LOGGER_FILE_NAME)
    logger.addHandler(file_handler)
    formatter = logging.Formatter("%(asctime)s - %(name)s - %(levelname)s - %(message)s")
    file_handler.setFormatter(formatter)

    trace_logger = logging.getLogger(_TRACE_LOGGER_NAME)
    trace_logger.setLevel(TRACE_LEVEL)
    trace_file_handler = logging.FileHandler(log_file_directory / _TRACE_LOGGER_FILE_NAME)
    trace_logger.addHandler(trace_file_handler)
    trace_file_handler.setFormatter(formatter)


def get_logger() -> logging.Logger:
    return logging.getLogger(_LOGGER_NAME)


def get_trace_logger() -> logging.Logger:
    return logging.getLogger(_TRACE_LOGGER_NAME)
